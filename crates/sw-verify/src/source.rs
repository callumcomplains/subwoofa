//! Comparing an encoded file with the audio it was made from: the decode is lined up with the source (an encoder
//! delays the audio by a known but format-dependent amount), then every channel is measured against its source
//! channel. A lossless stream must come back bit for bit; a lossy one gets a signal-to-noise figure per channel.

use crate::analysis::fft::fft;
use crate::{Check, Report, Status};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use sw_core::process::command;
use sw_core::{Channel, ChannelLayout};
use sw_pcm::{ChannelOrder, StemSet};

/// What the encoded file was made from.
pub struct Source<'a> {
    pub layout: &'static ChannelLayout,
    pub inputs: &'a HashMap<Channel, PathBuf>,
    pub order: ChannelOrder,
    pub start: Option<f64>,
    pub duration: Option<f64>,
    /// A delay (frames of silence in front, or of the start left out when negative) and a gain (dB) the encoder
    /// applied as it read the files, so the comparison sees the audio the encoder saw.
    pub delay_frames: i64,
    pub gain_db: Option<f64>,
    /// The decode is known to hold the layout's channels in FFmpeg's order: compare each channel with the decoded
    /// channel that order names, instead of matching them by content (which cannot tell apart two channels that carry
    /// nearly the same audio, and would hide a swap).
    pub order_known: bool,
}

/// How one channel compares.
#[derive(Debug, Clone, Serialize)]
pub struct ChannelMatch {
    /// The source channel, and the decoded channel that carries it.
    pub name: String,
    pub decoded_index: usize,
    /// Level of the source channel (RMS, dBFS), `None` for digital silence.
    pub rms_dbfs: Option<f64>,
    /// Signal to noise in dB (`None` when the channel is identical, or silent).
    pub snr_db: Option<f64>,
    /// The largest error, in dBFS (`None` when there is none).
    pub peak_error_dbfs: Option<f64>,
    /// Samples that differ when both are taken at 24 bits.
    pub mismatches: u64,
}

/// The result of comparing the decode with the source.
#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    /// How many samples later than the source the decoded audio starts (negative: earlier).
    pub lag_samples: i64,
    pub compared_frames: u64,
    pub source_frames: u64,
    pub decoded_frames: u64,
    /// Every channel is the same at 24 bits.
    pub exact: bool,
    pub channels: Vec<ChannelMatch>,
    /// The decoded channel order is not the one FFmpeg uses for the layout.
    pub reordered: bool,
}

/// Seconds decoded to find the lag.
const LAG_SECONDS: f64 = 12.0;
/// The most the decoded audio can be shifted from the source, either way.
const MAX_LAG: usize = 16_384;
/// Samples correlated.
const WINDOW: usize = 1 << 16;
/// Frames compared per block.
const BLOCK: usize = 1 << 15;
/// A correlation weaker than this is not the same programme.
const MIN_CORRELATION: f64 = 0.3;

const FULL_SCALE: f32 = 8_388_608.0;
/// Source channels quieter than this (RMS, dBFS) are left out of the lossy signal-to-noise figure.
const QUIET_DBFS: f64 = -60.0;

fn is_lossless(format: &str) -> bool {
    let f = format.to_ascii_lowercase();
    [
        "lossless",
        "master audio",
        "truehd",
        "mlp",
        "flac",
        "alac",
        "wavpack",
        "monkey",
        "tta",
        "als",
        "pcm",
        "wav",
        "aiff",
        "caf",
    ]
    .iter()
    .any(|k| f.contains(k))
        && !f.contains("lossy")
}

/// Starts FFmpeg decoding `path` to 32-bit float samples on its output, optionally only the first `seconds`.
fn spawn_decoder(path: &Path, ffmpeg: &Path, seconds: Option<f64>) -> Result<Decoder, String> {
    let mut cmd = command(ffmpeg);
    cmd.args(["-hide_banner", "-v", "error", "-i"]).arg(path);
    if let Some(s) = seconds {
        cmd.args(["-t", &format!("{s}")]);
    }
    let mut child = cmd
        .args(["-vn", "-f", "f32le", "-acodec", "pcm_f32le", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("FFmpeg could not be started: {e}"))?;
    // read on a thread of its own: a damaged file can make FFmpeg complain at length, and a full pipe would stall it
    let mut errors = child.stderr.take().unwrap();
    let complaints = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = errors.read_to_string(&mut text);
        text
    });
    Ok(Decoder { child, complaints })
}

/// FFmpeg decoding a file to its standard output, and what it says about problems on the way.
struct Decoder {
    child: std::process::Child,
    complaints: std::thread::JoinHandle<String>,
}

impl Decoder {
    fn out(&mut self) -> &mut std::process::ChildStdout {
        self.child.stdout.as_mut().unwrap()
    }

    /// Stops the decoder (it may still be running when enough was read) and returns its complaints, if any.
    fn finish(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.complaints.join().unwrap_or_default().trim().to_string()
    }
}

/// Reads up to `frames` frames of `channels` interleaved floats; fewer at the end of the stream.
fn read_frames(out: &mut impl Read, channels: usize, frames: usize) -> std::io::Result<Vec<Vec<f32>>> {
    let mut bytes = vec![0u8; frames * channels * 4];
    let mut have = 0;
    while have < bytes.len() {
        let n = out.read(&mut bytes[have..])?;
        if n == 0 {
            break;
        }
        have += n;
    }
    let got = have / (channels * 4);
    let mut planar = vec![Vec::with_capacity(got); channels];
    for frame in bytes[..got * channels * 4].chunks_exact(channels * 4) {
        for (ch, s) in frame.chunks_exact(4).enumerate() {
            planar[ch].push(f32::from_le_bytes([s[0], s[1], s[2], s[3]]));
        }
    }
    Ok(planar)
}

/// The shift of `y` against `x` that matches best (`y[n + lag] ~ x[n]`) and how well, as a correlation from 0 to 1.
fn best_lag(x: &[f32], y: &[f32], offset: usize) -> (i64, f64) {
    // y starts MAX_LAG before x does
    let size = (WINDOW + 2 * MAX_LAG + WINDOW).next_power_of_two();
    let (mut xr, mut xi) = (vec![0.0; size], vec![0.0; size]);
    let (mut yr, mut yi) = (vec![0.0; size], vec![0.0; size]);
    let mut ex = 0.0;
    for (i, v) in x[offset..offset + WINDOW].iter().enumerate() {
        xr[i] = *v as f64;
        ex += (*v as f64).powi(2);
    }
    let mut ey = 0.0;
    for i in 0..WINDOW + 2 * MAX_LAG {
        let at = (offset + i).checked_sub(MAX_LAG);
        if let Some(v) = at.and_then(|a| y.get(a)) {
            yr[i] = *v as f64;
            ey += (*v as f64).powi(2);
        }
    }
    if ex == 0.0 || ey == 0.0 {
        return (0, 0.0);
    }
    fft(&mut xr, &mut xi);
    fft(&mut yr, &mut yi);
    // correlation = IFFT(conj(X) * Y); the inverse transform by way of the forward one
    let (mut zr, mut zi) = (vec![0.0; size], vec![0.0; size]);
    for k in 0..size {
        let (a, b) = (xr[k], -xi[k]);
        let (c, d) = (yr[k], yi[k]);
        zr[k] = a * c - b * d;
        zi[k] = -(a * d + b * c);
    }
    fft(&mut zr, &mut zi);
    let scale = 1.0 / size as f64;
    let (mut best, mut at) = (f64::MIN, 0usize);
    for k in 0..=2 * MAX_LAG {
        let r = zr[k] * scale;
        if r > best {
            (best, at) = (r, k);
        }
    }
    (at as i64 - MAX_LAG as i64, best / (ex * ey).sqrt())
}

/// For each source channel the decoded channel that matches it at `lag`, by correlation over a stretch of audio.
fn match_channels(source: &[Vec<f32>], decoded: &[Vec<f32>], lag: i64, from: usize) -> Vec<usize> {
    let span = WINDOW.min(source[0].len().saturating_sub(from));
    let at = |ch: &Vec<f32>, i: usize| -> f64 {
        let j = (from + i) as i64 + lag;
        if j >= 0 {
            ch.get(j as usize).copied().unwrap_or(0.0) as f64
        } else {
            0.0
        }
    };
    let mut taken = vec![false; decoded.len()];
    source
        .iter()
        .enumerate()
        .map(|(s, sc)| {
            let mut best = (f64::MIN, s.min(decoded.len() - 1));
            for (d, dc) in decoded.iter().enumerate() {
                if taken[d] {
                    continue;
                }
                let (mut dot, mut es, mut ed) = (0.0, 0.0, 0.0);
                for i in 0..span {
                    let (a, b) = (sc[from + i] as f64, at(dc, i));
                    dot += a * b;
                    es += a * a;
                    ed += b * b;
                }
                let r = if es > 0.0 && ed > 0.0 {
                    dot / (es * ed).sqrt()
                } else {
                    -1.0 + (d == s) as i32 as f64 * 0.5
                };
                if r > best.0 {
                    best = (r, d);
                }
            }
            taken[best.1] = true;
            best.1
        })
        .collect()
}

/// The shift of the decode against the source, and where in the source it was measured: found on the loudest second of
/// the loudest source channel, against every decoded channel (the best match counts).
fn find_lag(source_head: &[Vec<f32>], head: &[Vec<f32>], rate: f64) -> Result<(i64, usize), String> {
    if source_head[0].len() < WINDOW + 1 {
        return Err("the source is too short to line up (it needs about two seconds)".into());
    }
    let energy = |c: &[f32], from: usize| {
        c[from..from + rate as usize]
            .iter()
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
    };
    if head[0].len() < WINDOW + 1 {
        return Err("the decode is too short to line up (it needs about two seconds)".into());
    }
    // the stretch must lie where both have audio: a decode that stops early (a damaged or cut file) is lined up on
    // what it has, and the comparison then says how much is missing
    let limit = source_head[0].len().min(head[0].len()).saturating_sub(WINDOW);
    let (mut best_window, mut best_channel, mut best_energy) = (0usize, 0usize, 0.0);
    for (ch, c) in source_head.iter().enumerate() {
        let mut from = 0;
        while from + (rate as usize) <= c.len() && from <= limit {
            let e = energy(c, from);
            if e > best_energy {
                (best_window, best_channel, best_energy) = (from, ch, e);
            }
            from += (rate / 4.0) as usize;
        }
    }
    if best_energy == 0.0 {
        return Err("the start of the source is silent, so the decode cannot be lined up with it".into());
    }
    let (mut lag, mut strength) = (0i64, 0.0);
    for d in head {
        let (l, r) = best_lag(&source_head[best_channel], d, best_window);
        if r > strength {
            (lag, strength) = (l, r);
        }
    }
    if strength < MIN_CORRELATION {
        return Err("the decode does not line up with the source (is it the same programme?)".into());
    }
    Ok((lag, best_window))
}

/// Decodes a whole file with FFmpeg into one vector of samples per channel, in FFmpeg's channel order.
pub fn decode_planar(path: &Path, ffmpeg: &Path) -> Result<Vec<Vec<f32>>, String> {
    let channels = decoded_width(path, ffmpeg).ok_or("FFmpeg could not read the encoded file")?;
    let mut decoder = spawn_decoder(path, ffmpeg, None)?;
    let mut planar = vec![Vec::new(); channels];
    loop {
        let block = read_frames(decoder.out(), channels, BLOCK).map_err(|e| e.to_string())?;
        for (all, part) in planar.iter_mut().zip(&block) {
            all.extend_from_slice(part);
        }
        if block[0].len() < BLOCK {
            break;
        }
    }
    decoder.finish();
    Ok(planar)
}

/// How much `decoded` is shifted against `source` (samples; positive means it starts late), both one vector per
/// channel and the decode in the same channel order as the source.
pub fn align(source: &[Vec<f32>], decoded: &[Vec<f32>], rate: u32) -> Result<i64, String> {
    let head = (LAG_SECONDS * rate as f64) as usize;
    let cut = |v: &[Vec<f32>], n: usize| v.iter().map(|c| c[..n.min(c.len())].to_vec()).collect::<Vec<_>>();
    find_lag(&cut(source, head), &cut(decoded, head + 2 * MAX_LAG), rate as f64).map(|(lag, _)| lag)
}

/// For each channel of `layout`, the channel of an FFmpeg decode that carries it (FFmpeg's own channel order, as
/// `sw_backend_ffmpeg::decode_order` gives it: a 7.1 decode puts the rear pair before the side pair).
pub fn decoded_order(layout: &ChannelLayout) -> Vec<usize> {
    let by_decoded = sw_backend_ffmpeg::decode_order(layout);
    let mut at = vec![0; by_decoded.len()];
    for (decoded, &channel) in by_decoded.iter().enumerate() {
        at[channel] = decoded;
    }
    at
}

/// Running totals of one channel.
#[derive(Default, Clone)]
struct Tally {
    signal: f64,
    error: f64,
    peak: f64,
    mismatches: u64,
    count: u64,
}

impl Tally {
    fn add(&mut self, source: &[f32], decoded: &[f32]) {
        for (&s, &d) in source.iter().zip(decoded) {
            let e = (d - s) as f64;
            self.count += 1;
            self.signal += (s as f64).powi(2);
            self.error += e * e;
            self.peak = self.peak.max(e.abs());
            if (s * FULL_SCALE).round() != (d * FULL_SCALE).round() {
                self.mismatches += 1;
            }
        }
    }
}

fn db(x: f64) -> Option<f64> {
    (x > 0.0).then(|| 10.0 * x.log10())
}

/// Compares the decode of `path` with `src` and adds the result to `report`: a check or two, and the figures.
pub fn compare_with_source(report: &mut Report, path: &Path, ffmpeg: &Path, src: &Source) -> Result<(), String> {
    let total = open_span(src)?.1;
    let alignment = line_up(path, ffmpeg, src, total)?;
    let run = tally_all(path, ffmpeg, src, total, &alignment)?;
    let comparison = Comparison {
        lag_samples: alignment.lag,
        compared_frames: run.compared,
        source_frames: total,
        decoded_frames: run.decoded_frames.saturating_sub(alignment.lag.max(0) as u64) + run.skipped_source,
        exact: run.tallies.iter().all(|t| t.mismatches == 0),
        reordered: alignment.mapping != decoded_order(src.layout),
        channels: run
            .tallies
            .iter()
            .enumerate()
            .map(|(i, t)| ChannelMatch {
                name: src.layout.channels[i].short().to_string(),
                decoded_index: alignment.mapping[i],
                rms_dbfs: db(t.signal / t.count.max(1) as f64),
                snr_db: if t.error > 0.0 { db(t.signal / t.error) } else { None },
                peak_error_dbfs: (t.peak > 0.0).then(|| 20.0 * t.peak.log10()),
                mismatches: t.mismatches,
            })
            .collect(),
    };
    add_checks(report, &comparison);
    if let Some(last) = run.complaints.lines().last() {
        // usually why a decode stopped early or came out short
        report.checks.push(Check {
            name: "Decoder messages".to_string(),
            status: Status::Warn,
            detail: format!("FFmpeg reported problems while decoding; the last: {last}"),
        });
    }
    report.comparison = Some(comparison);
    Ok(())
}

/// The source's files opened at the start of its span, and how many frames the span holds.
fn open_span(src: &Source) -> Result<(StemSet, u64), String> {
    let mut stems = StemSet::open_ordered(src.layout, src.inputs, src.order).map_err(|e| e.to_string())?;
    // the same order as the job runner: the delay first, then the gain measured on the delayed audio
    if src.delay_frames != 0 {
        stems.set_delay_frames(src.delay_frames);
    }
    if let Some(db) = src.gain_db {
        stems.set_gain_db(db);
    }
    let total = stems
        .select_span(src.start.unwrap_or(0.0), src.duration)
        .map_err(|e| e.to_string())?;
    if total == 0 {
        return Err("the source is empty".into());
    }
    Ok((stems, total))
}

/// How the decode sits against the source: how late it starts, and which decoded channel carries each source channel.
struct Alignment {
    lag: i64,
    mapping: Vec<usize>,
}

/// Lines the decode up with the source on the start of the programme.
fn line_up(path: &Path, ffmpeg: &Path, src: &Source, total: u64) -> Result<Alignment, String> {
    let (mut stems, _) = open_span(src)?;
    let rate = stems.sample_rate as f64;
    let channels = src.layout.channels.len();
    let first = (LAG_SECONDS * rate) as usize;
    let mut source_head = Vec::new();
    stems
        .read_planar(&mut source_head, first.min(total as usize))
        .map_err(|e| e.to_string())?;
    let mut decoder = spawn_decoder(path, ffmpeg, Some(LAG_SECONDS + MAX_LAG as f64 / rate + 1.0))?;
    let decoded_channels_n = decoded_width(path, ffmpeg).unwrap_or(channels);
    let head = read_frames(decoder.out(), decoded_channels_n, first + 2 * MAX_LAG).map_err(|e| e.to_string())?;
    // only the start is read here; what the decoder says about the rest comes out of the full pass
    decoder.finish();
    if decoded_channels_n != channels {
        return Err(format!(
            "the file decodes to {decoded_channels_n} channels but the source has {channels}"
        ));
    }
    let (lag, best_window) = find_lag(&source_head, &head, rate)?;
    Ok(Alignment {
        lag,
        mapping: if src.order_known {
            decoded_order(src.layout)
        } else {
            match_channels(&source_head, &head, lag, best_window)
        },
    })
}

/// What reading the whole decode against the whole source found.
struct Run {
    tallies: Vec<Tally>,
    compared: u64,
    decoded_frames: u64,
    /// Frames of the source before the decode starts (it started early), which have nothing to compare with.
    skipped_source: u64,
    /// What FFmpeg said went wrong while decoding (empty when nothing did).
    complaints: String,
}

/// Compares the whole decode with the whole source, block by block.
fn tally_all(path: &Path, ffmpeg: &Path, src: &Source, total: u64, alignment: &Alignment) -> Result<Run, String> {
    let (mut stems, _) = open_span(src)?;
    let channels = src.layout.channels.len();
    let mut decoder = spawn_decoder(path, ffmpeg, None)?;
    let mut planar = Vec::new();
    // a positive lag means the decode starts late: its first frames have no source
    let (mut skip_decoded, mut skip_source) = (alignment.lag.max(0) as usize, (-alignment.lag).max(0) as usize);
    let skipped_source = skip_source as u64;
    let mut tallies = vec![Tally::default(); channels];
    let (mut compared, mut decoded_frames) = (0u64, 0u64);
    // frames of the span not yet read
    let mut left = total;
    while skip_decoded > 0 {
        let n = skip_decoded.min(BLOCK);
        let got = read_frames(decoder.out(), channels, n).map_err(|e| e.to_string())?;
        decoded_frames += got[0].len() as u64;
        skip_decoded -= n;
        if got[0].len() < n {
            break;
        }
    }
    while skip_source > 0 {
        let n = skip_source.min(BLOCK);
        let got = stems.read_planar(&mut planar, n).map_err(|e| e.to_string())?;
        left = left.saturating_sub(got as u64);
        skip_source -= n;
        if got < n {
            break;
        }
    }
    loop {
        let want = BLOCK.min(left as usize);
        let got_source = stems.read_planar(&mut planar, want).map_err(|e| e.to_string())?;
        left -= got_source as u64;
        let decoded = read_frames(decoder.out(), channels, got_source.max(1)).map_err(|e| e.to_string())?;
        let got_decoded = decoded[0].len();
        decoded_frames += got_decoded as u64;
        let n = got_source.min(got_decoded);
        for (s, tally) in tallies.iter_mut().enumerate() {
            tally.add(&planar[s][..n], &decoded[alignment.mapping[s]][..n]);
        }
        compared += n as u64;
        if got_source < BLOCK || left == 0 || got_decoded < got_source {
            break;
        }
    }
    Ok(Run {
        tallies,
        compared,
        decoded_frames,
        skipped_source,
        complaints: decoder.finish(),
    })
}

/// How many channels the file decodes to.
fn decoded_width(path: &Path, ffmpeg: &Path) -> Option<usize> {
    crate::deep::probe_channels(path, ffmpeg).map(|(n, _)| n)
}

/// The checks that come out of a comparison.
fn add_checks(report: &mut Report, c: &Comparison) {
    // the structure check knows; a report made without one falls back on the format's name
    let named = |text: &str| is_lossless(text);
    let lossless = report
        .lossless
        .unwrap_or_else(|| named(&report.format) || report.summary.iter().any(|(k, v)| k == "Format" && named(v)));
    // a channel that is nearly silent in the source has no meaningful signal to noise
    let audible: Vec<&ChannelMatch> = c
        .channels
        .iter()
        .filter(|m| m.rms_dbfs.is_some_and(|r| r > QUIET_DBFS))
        .collect();
    let judged: Vec<&ChannelMatch> = if audible.is_empty() {
        c.channels.iter().collect()
    } else {
        audible
    };
    let worst = judged.iter().filter_map(|m| m.snr_db).fold(f64::INFINITY, f64::min);
    let mut push = |name: &str, status: Status, detail: String| {
        report.checks.push(Check {
            name: name.to_string(),
            status,
            detail,
        })
    };
    push(
        "Lined up with the source",
        Status::Info,
        format!(
            "the decode starts {} samples {} the source; {} frames compared",
            c.lag_samples.unsigned_abs(),
            if c.lag_samples >= 0 { "after" } else { "before" },
            c.compared_frames
        ),
    );
    if c.reordered {
        push(
            "Channel order",
            Status::Info,
            "the decoded channels are in a different order than FFmpeg's usual order for this layout (they were matched by content)".into(),
        );
    }
    let length_ok = c.decoded_frames >= c.source_frames;
    if c.exact && length_ok {
        push(
            "Identical to the source",
            Status::Pass,
            format!("all {} channels match the source sample for sample", c.channels.len()),
        );
    } else if lossless {
        let bad: u64 = c.channels.iter().map(|m| m.mismatches).sum();
        push(
            "Identical to the source (lossless)",
            Status::Fail,
            if !c.exact {
                format!("{bad} samples differ from the source")
            } else {
                format!(
                    "the stream is {} frames shorter than the source",
                    c.source_frames.saturating_sub(c.decoded_frames)
                )
            },
        );
    } else {
        let status = if worst >= 10.0 || !worst.is_finite() {
            Status::Info
        } else {
            Status::Warn
        };
        push(
            "Closeness to the source (lossy)",
            status,
            if worst.is_finite() {
                format!(
                    "signal to noise {worst:.1} dB in the worst channel (a lossy encode changes the audio on purpose)"
                )
            } else {
                "identical to the source".to_string()
            },
        );
        if !length_ok {
            push(
                "Length against the source",
                Status::Warn,
                format!(
                    "{} frames fewer than the source",
                    c.source_frames.saturating_sub(c.decoded_frames)
                ),
            );
        }
    }
    if matches!(report.checks.last().map(|c| c.status), Some(Status::Fail)) {
        report.passed = false;
    }
}
