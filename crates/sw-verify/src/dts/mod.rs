//! DTS: the core substream, the extension substream (DTS-HD) and the `.dtshd` container.
//!
//! A verification has three parts. `container` reads the chunks of a `.dtshd` file and finds the stream in it
//! (a raw `.dts` stream is the stream as it is). `walk` goes through the stream frame by frame, checking each core
//! frame and extension substream (`core_header`, `exss`, `xll`) and collecting facts. `report` turns the facts into
//! the report, and `container` checks what the chunks claim against them.

mod consts;
mod container;
mod core_header;
mod exss;
mod report;
mod walk;
mod xll;

use crate::{Collector, Probe, Status, Window};
use container::Container;
use report::Facts;
use walk::walk;

/// Verifies a DTS stream: a raw core / extension stream, or a `.dtshd` file when `container` is set.
pub(crate) fn verify(w: &mut Window, c: &mut Collector, container: bool) -> Probe {
    let mut p = Probe {
        dts: true,
        ..Default::default()
    };
    let found = if container {
        let found = container::read(w, c);
        if !found.stream_found {
            return p;
        }
        found
    } else {
        Container {
            end: w.len,
            ..Default::default()
        }
    };
    let s = walk(w, found.start, found.end, c, container);
    let facts = Facts::new(&s, found.aupr.as_ref());
    facts.fill_probe(&s, &mut p);
    report::describe(c, &s, &facts, found.end - found.start);
    report::check_structure(c, &s);
    if container && s.exss > 0 {
        report_peak(c, &p);
    }
    if container {
        found.cross_check(w, c, &s, &facts);
        if c.report.deep {
            found.deep_sections(c);
        }
    }
    if c.report.deep {
        report::deep_sections(c, &s);
    }
    p
}

/// Blu-ray's ceiling for a DTS-HD stream, core and extensions together, in kbps.
const BLURAY_KBPS: f64 = 24_500.0;

/// The busiest second of a DTS-HD stream against what a Blu-ray player has to accept.
fn report_peak(c: &mut Collector, p: &Probe) {
    if p.sample_rate == 0 || p.frame_bytes.is_empty() {
        return;
    }
    let measured = crate::peak_one_second(&p.frame_bytes, &p.frame_samples, p.sample_rate);
    c.kv("Peak bit rate (any 1 s)", format!("{measured:.0} kbps"));
    let (status, limit) = if measured > BLURAY_KBPS {
        (Status::Warn, "above")
    } else {
        (Status::Pass, "within")
    };
    c.check(
        "Peak data rate",
        status,
        format!("{measured:.0} kbps in the busiest second, {limit} the {BLURAY_KBPS:.0} kbps Blu-ray allows"),
    );
}
