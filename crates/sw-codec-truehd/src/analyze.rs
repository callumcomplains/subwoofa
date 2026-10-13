//! TrueHD / MLP stream analyser: walks access units the way a decoder does, without reconstructing
//! audio, to account for where the bits go and which coding tools a stream uses.

use crate::tables::HUFFMAN;
use std::collections::BTreeMap;
use std::fmt::Write;

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Bits { d, pos: 0 }
    }
    fn get(&mut self, n: usize) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.d.get(self.pos >> 3).copied().unwrap_or(0);
            v = (v << 1) | ((byte >> (7 - (self.pos & 7))) & 1) as u32;
            self.pos += 1;
        }
        v
    }
    fn get_signed(&mut self, n: usize) -> i32 {
        let v = self.get(n) as i32;
        if n == 0 {
            0
        } else {
            (v << (32 - n)) >> (32 - n)
        }
    }
    fn left(&self) -> usize {
        self.d.len() * 8 - self.pos.min(self.d.len() * 8)
    }
    fn peek_bit(&self, off: usize) -> u32 {
        let p = self.pos + off;
        ((self.d.get(p >> 3).copied().unwrap_or(0) >> (7 - (p & 7))) & 1) as u32
    }
}

#[derive(Clone, Default)]
struct Fir {
    order: u32,
    shift: u32,
    coeff_bits: u32,
    coeff_shift: u32,
    coeffs: Vec<i32>,
}

#[derive(Clone, Default)]
struct ChState {
    fir: Fir,
    iir_order: u32,
    huff_offset: i32,
    codebook: u32,
    huff_lsbs: u32,
}

#[derive(Clone, Default)]
struct SubState {
    seen_restart: bool,
    min: usize,
    max: usize,
    max_matrix: usize,
    noise_type: u32,
    presence: u32,
    blocksize: usize,
    quant: [u32; 16],
    ch: [ChState; 16],
    matrices: Vec<(u32, u32, u32)>, // (out channel, frac bits, lsb bypass)
}

#[derive(Default, Clone)]
pub struct SubStats {
    pub channels: String,
    pub bytes: u64,
    pub param_bits: u64,
    pub restart_bits: u64,
    pub data_bits: u64,
    pub huff_bits: u64,
    pub lsb_bits: u64,
    pub bypass_bits: u64,
    pub blocks: u64,
    pub param_blocks: u64,
    pub matrices: String,
    pub fir_orders: BTreeMap<u32, u64>,
    pub codebooks: BTreeMap<u32, u64>,
    pub lsbs: BTreeMap<u32, u64>,
    pub iir_orders: BTreeMap<u32, u64>,
    pub huff_offsets: u64,
    pub fir_updates: u64,
    pub iir_updates: u64,
    pub chan_updates: u64,
    pub fir_shifts: BTreeMap<u32, u64>,
    pub quant: BTreeMap<u32, u64>,
    /// per channel: (samples, residual bits)
    pub per_channel: BTreeMap<usize, (u64, u64)>,
}

#[derive(Default)]
pub struct Report {
    pub bytes: u64,
    pub access_units: u64,
    pub major_syncs: u64,
    pub major_interval: Vec<u64>,
    pub rate_code: u32,
    pub au_samples: u32,
    pub arrangement6: u32,
    pub arrangement8: u32,
    pub mods: (u32, u32, u32),
    pub substream_info: u32,
    pub peak_bitrate_field: u32,
    pub header_bytes: u64,
    pub subs: Vec<SubStats>,
    pub errors: Vec<String>,
    pub blocksizes: BTreeMap<usize, u64>,
}

const GROUPS: [&str; 13] = [
    "LR", "C", "LFE", "LRs", "LRvh", "LRc", "LRrs", "Cs", "Ts", "LRsd", "LRw", "Cvh", "LFE2",
];

fn groups(map: u32) -> String {
    (0..13)
        .filter(|g| map >> g & 1 == 1)
        .map(|g| GROUPS[g])
        .collect::<Vec<_>>()
        .join("+")
}

fn huff_symbol(b: &mut Bits, codebook: u32) -> Option<u32> {
    let table = &HUFFMAN[codebook as usize - 1];
    // codes are prefix-free: try increasing lengths
    let mut code = 0u32;
    for len in 1..=9u32 {
        code = (code << 1) | b.peek_bit(len as usize - 1);
        for (sym, (c, l)) in table.iter().enumerate() {
            if *l as u32 == len && *c as u32 == code {
                b.pos += len as usize;
                return Some(sym as u32);
            }
        }
    }
    None
}

pub fn analyze(data: &[u8]) -> Report {
    analyze_range(data, 0, usize::MAX)
}

/// Byte range of `count` restart intervals starting at the `first`-th major sync.
fn interval_range(data: &[u8], first: usize, count: usize) -> (usize, usize) {
    let mut pos = 0usize;
    let mut syncs = 0usize;
    let mut start = None;
    let mut end = data.len();
    while pos + 8 <= data.len() {
        let words = ((data[pos] & 0x0F) as usize) << 8 | data[pos + 1] as usize;
        if words == 0 {
            break;
        }
        if data[pos + 4..pos + 8] == [0xF8, 0x72, 0x6F, 0xBA] {
            if syncs == first {
                start = Some(pos);
            }
            if syncs == first.saturating_add(count) {
                end = pos;
                break;
            }
            syncs += 1;
        }
        pos += words * 2;
    }
    (start.unwrap_or(0), end)
}

/// Analyse only `count` restart intervals starting at the `first`-th major sync.
pub fn analyze_range(data: &[u8], first: usize, count: usize) -> Report {
    let (from, to) = interval_range(data, first, count);
    let data = &data[from..to];
    let mut r = Report {
        bytes: data.len() as u64,
        ..Default::default()
    };
    let mut states: Vec<SubState> = Vec::new();
    let mut pos = 0usize;
    let mut last_major_au = 0u64;
    while pos + 4 <= data.len() {
        let words = ((data[pos] & 0x0F) as usize) << 8 | data[pos + 1] as usize;
        let len = words * 2;
        if len < 4 || pos + len > data.len() {
            if pos + len > data.len() {
                break;
            }
            r.errors.push(format!("bad access unit length at byte {pos}"));
            break;
        }
        let au = &data[pos..pos + len];
        let mut off = 4;
        if au.len() >= 8 && au[4..8] == [0xF8, 0x72, 0x6F, 0xBA] {
            read_major_sync(au, &mut r, &mut states);
            off += 28;
            if r.major_syncs > 0 {
                r.major_interval.push(r.access_units - last_major_au);
            }
            last_major_au = r.access_units;
            r.major_syncs += 1;
        }
        pos += len;
        let nsub = states.len();
        if nsub != 0 {
            analyze_access_unit(au, pos - len, off, &mut r, &mut states);
        }
        r.access_units += 1;
    }
    r
}

/// The major sync: rate, channel arrangements and the number of substreams (which resets the per-substream state).
fn read_major_sync(au: &[u8], r: &mut Report, states: &mut Vec<SubState>) {
    let mut b = Bits::new(&au[4..32.min(au.len())]);
    b.pos = 32;
    r.rate_code = b.get(4);
    r.au_samples = 40 << (r.rate_code & 7);
    b.get(2);
    b.get(2);
    let m2 = b.get(2);
    let m6 = b.get(2);
    r.arrangement6 = b.get(5);
    let m8 = b.get(2);
    r.arrangement8 = b.get(13);
    r.mods = (m2, m6, m8);
    b.get(48);
    b.get(1);
    r.peak_bitrate_field = b.get(15);
    let nsub = b.get(4) as usize;
    b.get(4);
    r.substream_info = b.get(8);
    if states.len() != nsub {
        *states = vec![SubState::default(); nsub];
        r.subs = vec![SubStats::default(); nsub];
    }
}

/// The substream directory and then each substream of one access unit at byte `pos`, `off` being where the
/// directory starts in it.
fn analyze_access_unit(au: &[u8], pos: usize, mut off: usize, r: &mut Report, states: &mut [SubState]) {
    let nsub = states.len();
    let mut ends = Vec::new();
    for _ in 0..nsub {
        if off + 2 > au.len() {
            r.errors
                .push(format!("substream directory runs past the access unit at byte {pos}"));
            break;
        }
        let h = u16::from_be_bytes([au[off], au[off + 1]]);
        off += 2;
        if h >> 15 == 1 {
            off += 2;
        }
        ends.push((h & 0x0FFF) as usize * 2);
    }
    if ends.len() != nsub {
        return;
    }
    r.header_bytes += off as u64;
    let mut start = 0usize;
    // a fourth substream with the spatial-presentation flag carries Dolby Atmos object data, not channel audio
    let atmos = nsub == 4 && r.substream_info >> 7 == 1;
    for si in 0..nsub {
        let end = ends[si];
        let from = (off + start).min(au.len());
        let sub = &au[from..(off + end).min(au.len()).max(from)];
        r.subs[si].bytes += sub.len() as u64;
        if !(atmos && si == 3) {
            analyze_substream(
                sub,
                &mut states[si],
                &mut r.subs[si],
                &mut r.blocksizes,
                &mut r.errors,
                si,
            );
        }
        start = end;
    }
}

fn analyze_substream(
    data: &[u8],
    st: &mut SubState,
    stats: &mut SubStats,
    blocksizes: &mut BTreeMap<usize, u64>,
    errors: &mut Vec<String>,
    si: usize,
) {
    if data.len() < 2 {
        return;
    }
    let body = &data[..data.len() - 2]; // parity + checksum bytes
    let mut b = Bits::new(body);
    loop {
        let block_start = b.pos;
        if b.left() < 1 {
            break;
        }
        if b.get(1) == 1 {
            let restart = b.get(1) == 1;
            if restart {
                let s0 = b.pos;
                b.get(13);
                st.noise_type = b.get(1);
                b.get(16);
                st.min = b.get(4) as usize;
                st.max = b.get(4) as usize;
                st.max_matrix = b.get(4) as usize;
                b.get(4);
                b.get(23);
                b.get(19);
                b.get(1);
                b.get(8);
                b.get(16);
                for _ in 0..=st.max_matrix {
                    b.get(6);
                }
                b.get(8);
                stats.restart_bits += (b.pos - s0) as u64;
                if stats.channels.is_empty() {
                    stats.channels = format!("{}..{} (matrix ch {})", st.min, st.max, st.max_matrix);
                }
                st.seen_restart = true;
                st.presence = 0xFF;
                st.matrices.clear();
                st.blocksize = 8;
                st.quant = [0; 16];
                for c in &mut st.ch {
                    *c = ChState {
                        huff_lsbs: 24,
                        ..Default::default()
                    };
                }
            }
            if !st.seen_restart {
                return;
            }
            let p0 = b.pos;
            parse_params(&mut b, st, stats);
            stats.param_bits += (b.pos - p0) as u64;
            stats.param_blocks += 1;
        }
        if !st.seen_restart {
            return;
        }
        // block data
        *blocksizes.entry(st.blocksize).or_default() += 1;
        stats.blocks += 1;
        let d0 = b.pos;
        for _ in 0..st.blocksize {
            for (_, _, byp) in &st.matrices {
                if *byp == 1 {
                    b.get(1);
                    stats.bypass_bits += 1;
                }
            }
            for ch in st.min..=st.max {
                let cs = &st.ch[ch];
                let lsb = cs.huff_lsbs.saturating_sub(st.quant[ch]) as usize;
                let s = b.pos;
                if cs.codebook > 0 {
                    if huff_symbol(&mut b, cs.codebook).is_none() {
                        errors.push(format!("substream {si}: bad Huffman code"));
                        return;
                    }
                    stats.huff_bits += (b.pos - s) as u64;
                }
                b.get(lsb);
                stats.lsb_bits += lsb as u64;
                let e = stats.per_channel.entry(ch).or_default();
                e.0 += 1;
                e.1 += (b.pos - s) as u64;
            }
            if b.left() == 0 {
                errors.push(format!("substream {si}: ran out of data in a block"));
                return;
            }
        }
        stats.data_bits += (b.pos - d0) as u64;
        let _ = block_start;
        if b.get(1) == 1 {
            break;
        }
    }
}

fn parse_filter(b: &mut Bits, f: &mut Fir) {
    f.order = b.get(4);
    if f.order > 0 {
        f.shift = b.get(4);
        f.coeff_bits = b.get(5);
        f.coeff_shift = b.get(3);
        for _ in 0..f.order {
            f.coeffs.push(b.get_signed(f.coeff_bits as usize) << f.coeff_shift);
        }
        if b.get(1) == 1 {
            let sb = b.get(4) as usize;
            b.get(4);
            for _ in 0..f.order {
                b.get_signed(sb);
            }
        }
    }
}

fn parse_params(b: &mut Bits, st: &mut SubState, stats: &mut SubStats) {
    if st.presence & 1 != 0 && b.get(1) == 1 {
        st.presence = b.get(8);
    }
    if st.presence & 0x80 != 0 && b.get(1) == 1 {
        st.blocksize = b.get(9) as usize;
    }
    if st.presence & 0x40 != 0 && b.get(1) == 1 {
        let n = b.get(4);
        st.matrices.clear();
        let mut desc = String::new();
        for _ in 0..n {
            let out = b.get(4);
            let frac = b.get(4);
            let byp = b.get(1);
            let max_chan = st.max_matrix + if st.noise_type == 0 { 2 } else { 0 };
            let mut coeffs = Vec::new();
            for ch in 0..=max_chan {
                if b.get(1) == 1 {
                    let c = b.get_signed(frac as usize + 2) as f64 * (1u32 << (14 - frac.min(14))) as f64 / 16384.0;
                    coeffs.push(format!("c{ch}={c:.4}"));
                }
            }
            if st.noise_type == 1 {
                b.get(4);
            }
            let _ = write!(
                desc,
                "[out {out}{} {}] ",
                if byp == 1 { " +lsb" } else { "" },
                coeffs.join(" ")
            );
            st.matrices.push((out, frac, byp));
        }
        if stats.matrices.is_empty() {
            stats.matrices = desc;
        }
    }
    if st.presence & 0x20 != 0 && b.get(1) == 1 {
        for _ in 0..=st.max_matrix {
            b.get_signed(4);
        }
    }
    if st.presence & 0x10 != 0 && b.get(1) == 1 {
        for ch in 0..=st.max {
            st.quant[ch] = b.get(4);
            *stats.quant.entry(st.quant[ch]).or_default() += 1;
        }
    }
    for ch in st.min..=st.max {
        if b.get(1) == 1 {
            stats.chan_updates += 1;
            if st.presence & 8 != 0 && b.get(1) == 1 {
                let mut f = Fir::default();
                parse_filter(b, &mut f);
                st.ch[ch].fir = f;
                stats.fir_updates += 1;
            }
            if st.presence & 4 != 0 && b.get(1) == 1 {
                let mut f = Fir::default();
                parse_filter(b, &mut f);
                st.ch[ch].iir_order = f.order;
                stats.iir_updates += 1;
            }
            if st.presence & 2 != 0 && b.get(1) == 1 {
                st.ch[ch].huff_offset = b.get_signed(15);
            }
            st.ch[ch].codebook = b.get(2);
            st.ch[ch].huff_lsbs = b.get(5);
        }
        let c = &st.ch[ch];
        *stats.fir_orders.entry(c.fir.order).or_default() += 1;
        *stats.iir_orders.entry(c.iir_order).or_default() += 1;
        *stats.fir_shifts.entry(c.fir.shift).or_default() += 1;
        if c.huff_offset != 0 {
            stats.huff_offsets += 1;
        }
        *stats.codebooks.entry(c.codebook).or_default() += 1;
        *stats.lsbs.entry(c.huff_lsbs.saturating_sub(st.quant[ch])).or_default() += 1;
    }
}

impl Report {
    /// Human-readable summary.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let rate = match self.rate_code {
            0 => 48_000,
            1 => 96_000,
            2 => 192_000,
            8 => 44_100,
            9 => 88_200,
            10 => 176_400,
            _ => 0,
        };
        let _ = writeln!(
            s,
            "TrueHD stream: {:.1} MB, {} access units of {} samples at {} Hz ({:.1} s)",
            self.bytes as f64 / 1e6,
            self.access_units,
            self.au_samples,
            rate,
            self.access_units as f64 * self.au_samples as f64 / rate.max(1) as f64
        );
        let _ = writeln!(
            s,
            "Presentations: 6ch = {} ({}), 8ch = {} ({}); modifiers 2/6/8 = {:?}; substream info {:#04x}",
            self.arrangement6,
            groups(self.arrangement6),
            self.arrangement8,
            groups(self.arrangement8),
            self.mods,
            self.substream_info
        );
        let avg = if self.major_interval.is_empty() {
            0.0
        } else {
            self.major_interval.iter().sum::<u64>() as f64 / self.major_interval.len() as f64
        };
        let _ = writeln!(
            s,
            "Major syncs: {} (every {:.1} access units on average); AU/substream headers {:.2}% of the stream",
            self.major_syncs,
            avg,
            100.0 * self.header_bytes as f64 / self.bytes.max(1) as f64
        );
        let _ = writeln!(s, "Block sizes used: {:?}", self.blocksizes);
        for (i, sub) in self.subs.iter().enumerate() {
            let total_bits = (sub.bytes * 8).max(1) as f64;
            let _ = writeln!(
                s,
                "\nSubstream {i}: channels {}  {:.1} MB ({:.1}% of stream)",
                sub.channels,
                sub.bytes as f64 / 1e6,
                100.0 * sub.bytes as f64 / self.bytes.max(1) as f64
            );
            let _ = writeln!(s, "  bits: residual data {:.1}% (Huffman {:.1}%, raw LSBs {:.1}%, bypass {:.2}%), parameter blocks {:.2}% ({} of {} blocks), restart headers {:.2}%",
                100.0 * sub.data_bits as f64 / total_bits, 100.0 * sub.huff_bits as f64 / total_bits, 100.0 * sub.lsb_bits as f64 / total_bits, 100.0 * sub.bypass_bits as f64 / total_bits,
                100.0 * sub.param_bits as f64 / total_bits, sub.param_blocks, sub.blocks, 100.0 * sub.restart_bits as f64 / total_bits);
            if !sub.matrices.is_empty() {
                let _ = writeln!(s, "  matrices: {}", sub.matrices);
            }
            let hist = |m: &BTreeMap<u32, u64>| m.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>().join(" ");
            let _ = writeln!(
                s,
                "  FIR order histogram (per channel per params block): {}",
                hist(&sub.fir_orders)
            );
            let _ = writeln!(s, "  IIR order histogram: {}   FIR shift histogram: {}   nonzero huff offsets: {}   updates: channel {} fir {} iir {}", hist(&sub.iir_orders), hist(&sub.fir_shifts), sub.huff_offsets, sub.chan_updates, sub.fir_updates, sub.iir_updates);
            let _ = writeln!(s, "  codebook histogram: {}", hist(&sub.codebooks));
            let _ = writeln!(s, "  raw LSB count histogram: {}", hist(&sub.lsbs));
            let _ = writeln!(s, "  quantizer step histogram: {}", hist(&sub.quant));
            for (ch, (n, bits)) in &sub.per_channel {
                let _ = writeln!(
                    s,
                    "  channel {ch}: {:.2} bits/sample",
                    *bits as f64 / (*n).max(1) as f64
                );
            }
        }
        for e in &self.errors {
            let _ = writeln!(s, "ERROR: {e}");
        }
        s
    }
}
