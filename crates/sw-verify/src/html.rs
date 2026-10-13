//! The verification result as one self-contained HTML page, to attach to a delivery: the verdict, loudness, every
//! check, the bit rate chart and a spectrogram of each channel. It needs no network and prints cleanly.

use crate::{Analysis, Report, Spectrogram, Status};
use std::fmt::Write;

/// A true peak above this (dBTP) is flagged: lossy encoders can overshoot the level they are given.
const PEAK_WARNING_DBTP: f64 = -1.0;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn status_class(s: Status) -> &'static str {
    match s {
        Status::Pass => "pass",
        Status::Warn => "warn",
        Status::Fail => "fail",
        Status::Skipped => "skip",
        Status::Info => "info",
    }
}

fn status_word(s: Status) -> &'static str {
    match s {
        Status::Pass => "OK",
        Status::Warn => "WARN",
        Status::Fail => "FAIL",
        Status::Skipped => "skipped",
        Status::Info => "info",
    }
}

/// Today's date and time in UTC, `2026-10-13 14:05 UTC`.
fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // civil date from a day count (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        rem % 3600 / 60
    )
}

fn lufs(v: Option<f64>, unit: &str) -> String {
    v.map_or_else(|| format!("-inf {unit}"), |v| format!("{v:.1} {unit}"))
}

fn card(label: &str, value: String, flag: bool) -> String {
    format!(
        "<div class=\"card{}\"><span>{}</span><b>{}</b></div>",
        if flag { " flag" } else { "" },
        esc(label),
        esc(&value)
    )
}

impl Report {
    /// The report as a web page.
    pub fn to_html(&self) -> String {
        let mut h = String::new();
        let verdict = if self.passed { "PASSED" } else { "FAILED" };
        let _ = write!(
            h,
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>QC report: {}</title>\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><style>{CSS}</style></head><body>",
            esc(&self.file)
        );
        let _ = write!(
            h,
            "<header><div><h1>Subwoofa QC report</h1><p class=\"file\">{}</p><p class=\"meta\">{} · {} bytes · {} · {}</p></div>\
             <div class=\"verdict {}\">{}</div></header>",
            esc(&self.file),
            esc(&self.format),
            self.bytes,
            if self.deep { "deep verification" } else { "structure check" },
            utc_now(),
            if self.passed { "pass" } else { "fail" },
            verdict
        );
        if let Some(a) = &self.analysis {
            h.push_str(&loudness_block(a));
        }
        h.push_str("<h2>Summary</h2><table>");
        for (k, v) in &self.summary {
            let _ = write!(h, "<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v));
        }
        h.push_str("</table><h2>Checks</h2><table class=\"checks\">");
        for c in &self.checks {
            let _ = write!(
                h,
                "<tr class=\"{}\"><td class=\"tag\">{}</td><th>{}</th><td>{}</td></tr>",
                status_class(c.status),
                status_word(c.status),
                esc(&c.name),
                esc(&c.detail)
            );
        }
        h.push_str("</table>");
        if !self.findings.is_empty() {
            h.push_str("<h2>Findings</h2><table class=\"checks\">");
            for f in &self.findings {
                let at = f.offset.map(|a| format!(" (byte {a})")).unwrap_or_default();
                let _ = write!(
                    h,
                    "<tr class=\"{}\"><td class=\"tag\">{}</td><td>{}{}</td></tr>",
                    status_class(f.status),
                    status_word(f.status),
                    esc(&f.message),
                    at
                );
            }
            if self.findings_omitted > 0 {
                let _ = write!(h, "<tr><td></td><td>... and {} more</td></tr>", self.findings_omitted);
            }
            h.push_str("</table>");
        }
        if let Some(a) = &self.analysis {
            h.push_str(&peaks_table(a));
            h.push_str(&bitrate_chart(&a.bitrate_kbps));
            h.push_str(&spectrograms(&a.spectrograms));
        }
        if let Some(c) = &self.comparison {
            h.push_str(&comparison_table(c));
        }
        for s in &self.sections {
            let _ = write!(h, "<details><summary>{}</summary><table>", esc(&s.title));
            for (k, v) in &s.rows {
                let _ = write!(h, "<tr><th>{}</th><td>{}</td></tr>", esc(k), esc(v));
            }
            h.push_str("</table></details>");
        }
        let _ = write!(
            h,
            "<footer>Made by Subwoofa {}. Loudness follows ITU-R BS.1770-4 and EBU R 128.</footer>",
            esc(&sw_core::version::display())
        );
        h.push_str(SCRIPT);
        h.push_str("</body></html>");
        h
    }
}

fn loudness_block(a: &Analysis) -> String {
    let l = &a.loudness;
    let peak = l.true_peak();
    let over = peak.is_some_and(|p| p > PEAK_WARNING_DBTP);
    format!(
        "<h2>Loudness</h2><div class=\"cards\">{}{}{}{}{}</div>{}",
        card("Integrated", lufs(l.integrated, "LUFS"), false),
        card("Loudness range", lufs(l.range, "LU"), false),
        card("Short-term max", lufs(l.short_term_max, "LUFS"), false),
        card("Momentary max", lufs(l.momentary_max, "LUFS"), false),
        card("True peak", lufs(peak, "dBTP"), over),
        if over {
            format!(
                "<p class=\"note\">The true peak is above {PEAK_WARNING_DBTP} dBTP: a decoder may clip it. Measured over {:.0} s.</p>",
                l.seconds
            )
        } else {
            format!(
                "<p class=\"note\">Measured over {:.0} s of decoded audio.</p>",
                l.seconds
            )
        }
    )
}

fn peaks_table(a: &Analysis) -> String {
    let mut h = String::from(
        "<h2>Channels</h2><table class=\"peaks\"><tr><th>Channel</th><th>True peak</th><th>Sample peak</th></tr>",
    );
    for c in &a.loudness.channels {
        let _ = write!(
            h,
            "<tr><th>{}</th><td>{}</td><td>{}</td></tr>",
            esc(&c.name),
            lufs(c.true_peak, "dBTP"),
            lufs(c.sample_peak, "dBFS")
        );
    }
    h.push_str("</table>");
    h
}

/// How each channel compares with the source.
fn comparison_table(c: &crate::Comparison) -> String {
    let mut h = format!(
        "<h2>Against the source</h2><p class=\"note\">The decode starts {} samples {} the source; {} frames compared{}.</p>         <table class=\"peaks\"><tr><th>Channel</th><th>Signal to noise</th><th>Largest error</th><th>Differing samples</th></tr>",
        c.lag_samples.unsigned_abs(),
        if c.lag_samples >= 0 { "after" } else { "before" },
        c.compared_frames,
        if c.exact { ", identical at 24 bits" } else { "" }
    );
    for m in &c.channels {
        let _ = write!(
            h,
            "<tr><th>{}</th><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&m.name),
            m.snr_db
                .map_or_else(|| "identical".to_string(), |v| format!("{v:.1} dB")),
            m.peak_error_dbfs
                .map_or_else(|| "none".to_string(), |v| format!("{v:.1} dBFS")),
            m.mismatches
        );
    }
    h.push_str("</table>");
    h
}

/// The bit rate over time as an inline SVG.
fn bitrate_chart(series: &[f32]) -> String {
    if series.len() < 2 {
        return String::new();
    }
    let (w, ht) = (800.0, 160.0);
    let top = series.iter().copied().fold(0.0f32, f32::max).max(1.0) * 1.05;
    let mean = series.iter().sum::<f32>() / series.len() as f32;
    let points: String = series
        .iter()
        .enumerate()
        .map(|(i, v)| {
            format!(
                "{:.1},{:.1} ",
                i as f32 / (series.len() - 1) as f32 * w,
                ht - v / top * ht
            )
        })
        .collect();
    format!(
        "<h2>Bit rate</h2><svg viewBox=\"0 0 {w} {ht}\" class=\"chart\" preserveAspectRatio=\"none\">\
         <polyline points=\"{points}\" fill=\"none\" stroke=\"#2563eb\" stroke-width=\"1.5\"/></svg>\
         <p class=\"note\">Mean {mean:.0} kbps, highest second {:.0} kbps, over {} seconds.</p>",
        series.iter().copied().fold(0.0f32, f32::max),
        series.len()
    )
}

fn spectrograms(list: &[Spectrogram]) -> String {
    if list.is_empty() {
        return String::new();
    }
    let mut h = String::from("<h2>Spectrograms</h2><p class=\"note\">Peak level per band over time, 0 dBFS (white) to -100 dBFS (black); frequency on a log scale.</p><div class=\"spectros\">");
    for s in list {
        let _ = write!(
            h,
            "<figure><canvas data-bands=\"{}\" data-columns=\"{}\" data-levels=\"{}\" width=\"{}\" height=\"{}\"></canvas>\
             <figcaption><b>{}</b> · {:.0} Hz to {:.0} kHz · {:.0} s</figcaption></figure>",
            s.bands,
            s.columns,
            s.levels,
            s.columns,
            s.bands,
            esc(&s.channel),
            s.low_hz,
            s.high_hz / 1000.0,
            s.seconds
        );
    }
    h.push_str("</div>");
    h
}

const CSS: &str = "\
body{font:14px/1.45 system-ui,Segoe UI,Helvetica,Arial,sans-serif;color:#1b1f24;background:#fff;max-width:960px;margin:24px auto;padding:0 20px}\
header{display:flex;justify-content:space-between;align-items:flex-start;gap:16px;border-bottom:2px solid #1b1f24;padding-bottom:12px}\
h1{margin:0 0 4px;font-size:22px}h2{margin:26px 0 8px;font-size:16px;border-bottom:1px solid #d0d7de;padding-bottom:4px}\
.file{margin:0;font:600 13px ui-monospace,Consolas,monospace;word-break:break-all}.meta{margin:2px 0 0;color:#57606a}\
.verdict{font-weight:800;letter-spacing:.08em;padding:8px 16px;border-radius:4px;color:#fff}.verdict.pass{background:#1a7f37}.verdict.fail{background:#cf222e}\
table{border-collapse:collapse;width:100%}th,td{text-align:left;padding:3px 8px;border-bottom:1px solid #eaeef2;vertical-align:top}th{font-weight:600;color:#424a53;white-space:nowrap}\
.checks .tag{width:60px;font:700 11px ui-monospace,Consolas,monospace;white-space:nowrap}.checks .pass .tag{color:#1a7f37}.checks .fail .tag{color:#cf222e}.checks .warn .tag{color:#9a6700}.checks .info .tag,.checks .skip .tag{color:#57606a}\
.cards{display:flex;flex-wrap:wrap;gap:10px}.card{border:1px solid #d0d7de;border-radius:6px;padding:8px 14px;min-width:130px}.card span{display:block;color:#57606a;font-size:12px}.card b{font-size:20px}.card.flag{border-color:#bf8700;background:#fff8c5}\
.note{color:#57606a;margin:6px 0}.peaks{width:auto}.chart{width:100%;height:160px;border:1px solid #d0d7de;background:#f6f8fa}\
.spectros{display:grid;grid-template-columns:repeat(auto-fill,minmax(300px,1fr));gap:12px}figure{margin:0}canvas{width:100%;height:150px;image-rendering:auto;border:1px solid #d0d7de;background:#000}figcaption{font-size:12px;color:#57606a}\
details{margin:10px 0}summary{cursor:pointer;font-weight:600}footer{margin-top:30px;color:#57606a;font-size:12px;border-top:1px solid #d0d7de;padding-top:8px}\
@media print{body{margin:0;max-width:none}details{break-inside:avoid}details>*{display:block}}";

/// Draws each spectrogram from the bytes in its data attribute.
const SCRIPT: &str = "<script>\
const stops=[[0,0,4],[87,16,110],[188,55,84],[249,142,9],[252,255,164]];\
function colour(v){const t=v/255*(stops.length-1),i=Math.min(stops.length-2,Math.floor(t)),f=t-i;return stops[i].map((c,k)=>Math.round(c+(stops[i+1][k]-c)*f));}\
document.querySelectorAll('canvas[data-levels]').forEach(cv=>{\
const bands=+cv.dataset.bands,cols=+cv.dataset.columns,raw=atob(cv.dataset.levels),ctx=cv.getContext('2d'),img=ctx.createImageData(cols,bands);\
for(let x=0;x<cols;x++)for(let b=0;b<bands;b++){const v=raw.charCodeAt(x*bands+b),c=colour(v),o=((bands-1-b)*cols+x)*4;img.data[o]=c[0];img.data[o+1]=c[1];img.data[o+2]=c[2];img.data[o+3]=255;}\
ctx.putImageData(img,0,0);});\
</script>";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Check;

    #[test]
    fn the_page_shows_the_verdict_and_escapes_names() {
        let report = Report {
            file: "<b>mix</b>.dts".into(),
            format: "DTS".into(),
            passed: false,
            checks: vec![Check {
                name: "Sync words".into(),
                status: Status::Fail,
                detail: "lost & found".into(),
            }],
            ..Default::default()
        };
        let page = report.to_html();
        assert!(page.contains("FAILED") && page.contains("&lt;b&gt;mix&lt;/b&gt;.dts"));
        assert!(page.contains("lost &amp; found") && page.contains("class=\"fail\""));
        assert!(!page.contains("<b>mix</b>"));
    }

    #[test]
    fn the_date_is_a_plausible_utc_time() {
        let now = utc_now();
        assert!(now.starts_with("20") && now.ends_with(" UTC"), "{now}");
    }
}
