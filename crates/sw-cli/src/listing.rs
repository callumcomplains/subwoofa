//! The small read-only commands: `layouts`, `formats`, `info` and `stream-info`.

use std::path::Path;
use sw_core::formats::Destination;
use sw_core::LAYOUTS;

pub(crate) fn layouts() {
    for l in LAYOUTS {
        println!("{:<22} {:<4} {}", l.id, l.short_label(), l.name);
    }
}

pub(crate) fn formats() {
    for d in Destination::ALL {
        println!("{} ({})", d.display_name(), d.id());
        for s in d.stream_types() {
            println!("    {:<12} {} [.{}]", s.id(), s.display_name(), d.extension(*s));
        }
    }
}

/// Format of a WAV/AIFF file and, when a span is asked for, its RMS level (to cross-check other readers).
pub(crate) fn info(path: &Path, start: Option<f64>, duration: Option<f64>) -> Result<(), String> {
    let mut reader = sw_pcm::PcmReader::open(path).map_err(|e| e.to_string())?;
    let i = reader.info().clone();
    println!(
        "{:?}  {} ch  {} Hz  {}-bit{}  {} frames ({:.3} s)",
        i.container,
        i.channels,
        i.sample_rate,
        i.bits,
        if i.float { " float" } else { "" },
        i.frames,
        i.duration_secs()
    );
    if let Some(t) = i.start_secs() {
        println!("broadcast start: {t:.3} s since midnight");
    }
    if start.is_some() || duration.is_some() {
        reader
            .seek_frame((start.unwrap_or(0.0) * i.sample_rate as f64) as u64)
            .map_err(|e| e.to_string())?;
        let want = (duration.unwrap_or(10.0) * i.sample_rate as f64) as usize;
        let (mut sum, mut n, mut buf) = (0f64, 0usize, Vec::new());
        while n < want {
            buf.clear();
            let got = reader
                .read_f32(&mut buf, (want - n).min(1 << 16))
                .map_err(|e| e.to_string())?;
            if got == 0 {
                break;
            }
            sum += buf.iter().map(|v| (*v as f64).powi(2)).sum::<f64>();
            n += got * i.channels as usize;
        }
        println!("RMS over span: {:.3} dBFS", 10.0 * (sum / n.max(1) as f64).log10());
    }
    Ok(())
}

/// Structure and bit use of a TrueHD stream.
pub(crate) fn stream_info(path: &Path, first: usize, count: Option<usize>) -> Result<(), String> {
    let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let analysis = sw_codec_truehd::analyze::analyze_range(&data, first, count.unwrap_or(usize::MAX));
    print!("{}", analysis.summary());
    Ok(())
}
