//! DTS-HD Master Audio with a core: a CBR core in the core substream plus the lossless residual of the
//! core in an extension substream.

use crate::encoder::{Config as CoreConfig, Encoder as CoreEncoder};
use crate::exss::{exss_frame, ExssParams};
use crate::ma::map_layout;
use crate::native::map_layout as core_map_layout;
use crate::xll::{XllConfig, XllEncoder};
use sw_core::ChannelLayout;

pub struct MaCoreConfig {
    pub layout: &'static ChannelLayout,
    pub sample_rate: u32,
    pub bit_res: u32,
    pub core_bitrate: u32,
    pub dialnorm: i32,
}

/// Core (CBR, lossy) plus lossless residual, emitted frame by frame.
pub struct MaCoreEncoder {
    core: CoreEncoder,
    xll: XllEncoder,
    exss: ExssParams,
    /// Source samples per XLL channel (coded order), not yet fully consumed.
    buf: Vec<Vec<i32>>,
    /// Core slot (primary channels in stream order, then LFE) for every XLL channel.
    xll_to_core: Vec<usize>,
    n: usize,
    delay: usize,
    emitted: usize,
    core_pad: usize,
    shift: u32,
}

impl MaCoreEncoder {
    pub fn new(cfg: MaCoreConfig) -> Result<MaCoreEncoder, crate::Error> {
        let cmap = core_map_layout(cfg.layout)?;
        let xmap = map_layout(cfg.layout);
        let nprim = cmap.order.len();
        let mut core = CoreEncoder::new(CoreConfig {
            sample_rate: cfg.sample_rate,
            amode: cmap.amode,
            channels: nprim,
            lfe: cmap.lfe.is_some(),
            bitrate: cfg.core_bitrate,
            dialnorm: cfg.dialnorm,
            pcm_bits: cfg.bit_res,
            es: false,
            blocks: 16,
        })?;
        core.enable_exact_decode();
        let n = core.frame_samples();
        let nch = xmap.order.len();
        let xll_to_core: Vec<usize> = xmap
            .order
            .iter()
            .map(|&lc| cmap.order.iter().position(|&o| o == lc).or_else(|| (cmap.lfe == Some(lc)).then_some(nprim)).expect("layout channel missing from the core"))
            .collect();
        let xll = XllEncoder::new(XllConfig { sample_rate: cfg.sample_rate, channels: nch, ch_mask: xmap.xll_mask, bit_res: cfg.bit_res, residual_of_core: true, frame_samples: n });
        let exss = ExssParams { sample_rate: cfg.sample_rate, channels: nch, bit_res: cfg.bit_res, spk_mask: xmap.exss_mask, with_core: true, dialnorm: 0, frame_duration_code: 0 };
        let delay = core.delay();
        let core_pad = (4 - core.frame_bytes() % 4) % 4;
        Ok(MaCoreEncoder { core, xll, exss, buf: vec![Vec::new(); nch], xll_to_core, n, delay, emitted: 0, core_pad, shift: 24 - cfg.bit_res })
    }

    pub fn frame_samples(&self) -> usize {
        self.n
    }

    /// Feed `frame_samples()` source samples per XLL channel; returns the encoded frames that became ready.
    pub fn push(&mut self, frame: &[Vec<i32>]) -> Vec<Vec<u8>> {
        for (b, f) in self.buf.iter_mut().zip(frame) {
            b.extend_from_slice(f);
        }
        let mut out = Vec::new();
        while self.buf[0].len() >= (self.emitted + 1) * self.n + self.delay {
            out.push(self.emit());
        }
        out
    }

    /// Flush: pad with silence so every pushed sample is covered by an emitted frame.
    pub fn finish(&mut self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let pushed = self.buf[0].len();
        let frames = pushed.div_ceil(self.n);
        let want = frames * self.n + self.delay;
        for b in self.buf.iter_mut() {
            b.resize(want.max(b.len()), 0);
        }
        while self.emitted < frames {
            out.push(self.emit());
        }
        out
    }

    fn emit(&mut self) -> Vec<u8> {
        let n = self.n;
        let start = self.emitted * n;
        let nch = self.buf.len();
        let ncore = self.xll_to_core.iter().copied().max().map_or(0, |m| m + 1);
        // core input: the source advanced by the filter bank delay, in 24-bit units
        let mut core_in: Vec<Vec<f64>> = vec![Vec::new(); ncore];
        for (x, &slot) in self.xll_to_core.iter().enumerate() {
            core_in[slot] = self.buf[x][start + self.delay..start + self.delay + n].iter().map(|v| *v as f64 * (1u64 << self.shift) as f64).collect();
        }
        let core_bytes = self.core.encode_frame(&core_in);
        // residual against the reference decoder's output
        let round = if self.shift > 0 { 1i32 << (self.shift - 1) } else { 0 };
        let mut resid: Vec<Vec<i32>> = Vec::with_capacity(nch);
        for (x, &slot) in self.xll_to_core.iter().enumerate() {
            let c = &self.core.last_core_pcm[slot];
            resid.push((0..n).map(|i| self.buf[x][start + i] - ((c[i] + round) >> self.shift)).collect());
        }
        let xll = self.xll.encode_frame(&resid);
        let exss = exss_frame(&self.exss, &xll);
        let mut out = core_bytes;
        out.resize(out.len() + self.core_pad, 0);
        out.extend_from_slice(&exss);
        self.emitted += 1;
        out
    }
}
