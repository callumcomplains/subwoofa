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
