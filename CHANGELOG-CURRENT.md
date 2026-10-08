# Current changelog (running log of work done while you were away)

Newest entries at the bottom. Started 2026-10-08.

## Baseline (before this log)
- Native DTS-HD MA (core + XLL) lossless for 5.1 at 44.1/48/88.2/96 kHz; full Oppenheimer film verified bit-exact, passes DTSHDVerify, ~5% smaller than DTSHDMASE.
- Native TrueHD (within ~2.7% of Dolby size), AC-3, DTS core.

## Plan order
1. 7.1 DTS-HD MA  2. 96 kHz MA size gap  3. GUI options for MA  4. ES/XCH, 96/24, HRA, Express  5. DVD-Audio MLP, AC-3 improvements, StreamTools tab, speed  6. Docs/commit

## Log
- Roadmap: added NHK 22.2 (24-channel) layout as item 6 (awaiting your test files).
- **7.1 DTS-HD MA (done).** Two XLL channel sets in a hierarchy: set 0 = the 5.1 base (C L R LFE + side pair), set 1 = the two extra channels (rears / heights / wides), with an embedded 5.1 downmix (extras folded in at -3 dB, scale from the DTS table). The core carries the 5.1 downmix; the lossless extension carries its residual plus the extra channels. Verified: bit-exact against a 60 s excerpt of Terrifier 3 8ch (FFmpeg decode), `DTSHDVerify` passes, and a 5.1-only decoder gets the exact documented downmix (test `seven_one_embedded_five_one_downmix`). Works for every 7.1 layout whose extras fold into the base (Lsr/Rsr, Lss/Rss, Lh/Rh, Lhs/Rhs, Lw/Rw). Layouts 7.1 Cs/Ch and Cs/Oh are lossless-only for now (single 8-channel set, no core).
  - Note: FFmpeg's decoder starts multi-set streams on the lossy core downmix, so the very first frame (10 ms) of a 7.1 stream decodes as the core; that is decoder behaviour, not an encoder error.
- **96 kHz MA size gap closed (done).** Root cause: the 48 kHz core was fed 487 samples (96 kHz) out of alignment, so the decoder's reconstruction of the core cancelled almost nothing and the lossless extension was carrying nearly the whole signal. The earlier alignment sweep had settled in a false local minimum. Correct alignment is lookahead 480 core-rate samples with a half-sample decimation phase (the 64-band synthesis has a half-sample group delay, so the decimating filter now supports a fractional phase). Result on a 40-frame slice of Sultans of Swing (96 kHz): 9500 bytes of lossless data per frame against 9616 for DTSHDMASE (about 1% smaller; was 18% larger). 30 s excerpt is bit-exact through FFmpeg and passes DTSHDVerify; encoding is also ~4x faster than before (1.1x real time at 96 kHz).
- Debug aid: `SW_DUMP_RESID=<file>` makes the MA encoder write each frame's core residual (i32) for analysis.
