//! How hard the encoder looks for a smaller block.

/// Highest linear predictor order tried.
/// What the encoder tries for each block. More of each takes longer and saves a little.
#[derive(Clone, Copy)]
pub(crate) struct Search {
    /// Orders of the linear predictors fitted to a block.
    pub(crate) lpc_orders: &'static [usize],
    /// Highest order of the fixed (polynomial) predictors tried, 0 to 4.
    pub(crate) fixed_max: usize,
    /// The block is cut into up to `2^max_partition` runs, each with its own Rice parameter.
    pub(crate) max_partition: u32,
    /// Code a stereo pair every way and keep the smallest; otherwise pick the way that looks best from the signals.
    pub(crate) all_stereo: bool,
}

impl Search {
    pub(crate) fn for_effort(effort: sw_core::Effort) -> Search {
        match effort {
            sw_core::Effort::Best => Search {
                lpc_orders: &[2, 4, 8, 12],
                fixed_max: 4,
                max_partition: 8,
                all_stereo: true,
            },
            sw_core::Effort::Balanced => Search {
                lpc_orders: &[4, 8, 12],
                fixed_max: 2,
                max_partition: 6,
                all_stereo: false,
            },
            sw_core::Effort::Fast => Search {
                lpc_orders: &[8],
                fixed_max: 1,
                max_partition: 5,
                all_stereo: false,
            },
        }
    }

    /// Highest order the Levinson-Durbin recursion has to reach.
    pub(crate) fn top_order(&self) -> usize {
        self.lpc_orders.iter().copied().max().unwrap_or(0)
    }
}

pub(crate) const LPC_PRECISION: u32 = 15;
