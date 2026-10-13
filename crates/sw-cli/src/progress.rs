//! A progress line on stderr while a command runs.

use crate::tools::clock;

/// Progress on the terminal: percentage, time spent and time left, redrawn a few times a second. Prints nothing when
/// the output is not a terminal (a log file, a pipe).
pub(crate) struct Progress {
    pub(crate) started: std::time::Instant,
    pub(crate) last_drawn: std::time::Instant,
    pub(crate) drawn_anything: bool,
    pub(crate) enabled: bool,
}

impl Progress {
    pub(crate) fn new() -> Progress {
        use std::io::IsTerminal;
        let now = std::time::Instant::now();
        Progress {
            started: now,
            last_drawn: now,
            drawn_anything: false,
            enabled: std::io::stderr().is_terminal(),
        }
    }

    /// `fraction` of the work done, 0.0 to 1.0.
    pub(crate) fn tick(&mut self, fraction: f32) {
        if fraction >= 0.999 {
            // finished: leave a clean line for what the command prints next
            self.clear();
            return;
        }
        if !self.enabled || self.last_drawn.elapsed().as_millis() < 250 || fraction <= 0.0 {
            return;
        }
        self.last_drawn = std::time::Instant::now();
        let spent = self.started.elapsed().as_secs_f32();
        let left = spent * (1.0 - fraction) / fraction;
        eprint!(
            "\r  {:3.0}%   {} spent, about {} left   ",
            fraction * 100.0,
            clock(spent),
            clock(left)
        );
        self.drawn_anything = true;
    }

    /// Wipes the progress line.
    fn clear(&mut self) {
        if self.drawn_anything {
            eprint!("\r{:60}\r", "");
            self.drawn_anything = false;
        }
    }
}
