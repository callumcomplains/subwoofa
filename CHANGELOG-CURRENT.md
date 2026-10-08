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
- **GUI/CLI: MA core switch (done).** DTS-HD MA now has a "Backward-compatible core" checkbox in the Audio tab (core bit rate already followed the Bit rate field). Unticking it writes the lossless extension only (bit rate 0 in the job spec / `--bitrate 0` on the CLI). Verified on 7.1: lossless via FFmpeg, passes DTSHDVerify.
- **DTS-ES native (done): Matrix and Discrete.**
  - *6.1/6.0 Matrix:* the centre surround is mixed into Ls/Rs at -3 dB and the ES flag is set in the core header; "pre-mixed" layouts only set the flag. Options **ES phase shift** (surrounds rotated by +/-45 degrees before the Cs is added, via a windowed-FIR Hilbert transformer; every channel is delayed 127 samples so they stay aligned) and **Rear channels -3 dB** are available in the CLI (`--es-phase-shift`, `--rear-3db`), the job spec and the GUI (new "DTS options" row for DTS streams).
  - *6.1/6.0 Discrete (XCH extension):* the sixth channel (Cs) is coded as a second channel set at the tail of every core frame (sync 0x5A5A5A5A, own header and side information, placed so it ends exactly at the end of the frame); the core carries the 5.1 downmix with Cs at -3 dB. Rate control accounts for the extension block (including its 32-bit alignment and the 1024-byte limit of XChFSIZE). FFmpeg identifies the result as "DTS-ES 6.1", un-mixes the surrounds correctly, and decodes the Cs channel at 55 dB SNR; DTSHDVerify passes. New test `es_discrete_roundtrip`.
  - Raw `.dts` core output only so far (the DVD `.cpt` extension is a plain rename of the same stream).
- Test fix: removed a dead overflowing line from the band-mapping test.
- **DTS-HD High Resolution (HRA) native (done): core + XBR extension, 48 kHz, up to 5.1.** New `xbr` module: the extension carries a finer second quantization of what the core left over, in the subband domain (reverse water-filling with a Lagrangian rate search, block-coded or linear residuals with the lossless step table, 7-bit scale factors, own frame header with CRC16). It travels in an extension substream (`exss_frame_ext`, mask 0x21 = core in core substream + XBR) next to the 1509 kbit/s core, padded to a constant total bit rate (the Bit rate field now means the total, 2046-5376 kbps). Verified with FFmpeg ("DTS-HD HRA"): on 20 s of Oppenheimer at 3018 kbps the decoded error is at the 24-bit float-decoder floor (core alone 60-70 dB SNR, with XBR 85-107 dB depending on the channel); DTSHDVerify passes; new test `xbr_improves_on_the_core` (stereo 2500 kbps: core 94 dB, with XBR 117 dB).
  - The HRA core uses the perfect-reconstruction filter bank (FILTS flag) so the extension can fully refine it.
  - Bug found and fixed on the way: the perfect-reconstruction filter-bank table I had generated from the spec text was slightly off (up to 6e-6, large relative to the small taps) and capped PR streams at 68 dB; replaced by the reference table (the non-perfect one was already exact). MA/DTS cores use the non-perfect bank and were not affected.
  - Not yet: HRA at 96 kHz (needs X96), HRA of 6.1 discrete (XCH + XBR) and 7.1 (needs XXCH).
- **DTS 96/24 native (done): core + X96 extension, 96 kHz input, raw `.dts`/`.cpt`.** New pieces: a 64-band analysis bank built from the decoder's own (bit-exact) 64-band synthesis (`qmf64`, 84 dB analysis-synthesis SNR), the X96 coder (`x96`: frame/channel-set header, side information, 6-bit scale factors, block/plain quantizers from the core's families, water-filling bit sharing up to a byte budget) and the driver (`dts96`, `native96`) that feeds the 48 kHz core from the half-band-decimated programme (same calibrated alignment as the 96 kHz MA path) and gives the extension the lower-half residual (analysis of the 96 kHz programme minus what the core decoder reconstructs) plus the upper 32 bands as they are. A fifth of every frame goes to X96 (the rest to the core); the extension sits at the tail of the frame like XCH and the core header flags it (EXT_AUDIO_ID = 2).
  - Verified with FFmpeg ("DTS 96/24", 96000 Hz, no decode errors) and DTSHDVerify; 12 s of Sultans of Swing decodes at 39-47 dB SNR on the full-band channels; test `tone_above_24_khz_survives` shows a 31 kHz tone (impossible for a plain core) coming back at 100% of its level.
  - Speed is about 1x real time for 5.1 at 96 kHz (the 64-band analysis is a direct matched filter; a polyphase version would be several times faster).
  - Not yet: 96/24 with 6.x ES layouts, X96 inside MA (the "96/24 Core" option), Huffman-coded side information (the extension currently spends a little more on side info than necessary).
- **MLP / DVD-Audio native (done).** The lossless TrueHD engine now also writes MLP: MLP major sync (format info, channel assignment, DVD-Audio flag, channel meaning), one substream, no downmix, 32-bit end-of-stream marker, assignments 0-12 (mono, stereo, L-R-Cs, quad, 2.1, 3.0, 4.0, 5.0, 3.1, 4.1, 5.1) with their coded channel order, 16/20/24-bit word length, 44.1-192 kHz. The `mlp` stream type is native in the CLI, queue and GUI. Verified: FFmpeg decodes it bit-exactly (5.1 at 48 and 96 kHz on real material, stereo 16-bit 44.1 kHz and 5.1 24-bit 96 kHz in tests `mlp_stereo_16_bit_44_1` / `mlp_five_one_24_bit_96`). Raw `.mlp` stream only (no DVD-Audio disc authoring).
- **StreamTools (done, first version).** New crate `sw-tools` + a "StreamTools" mode in the GUI (switch in the header) + `subwoofa tools ...` on the CLI. It indexes DTS / DTS-HD (raw `.dts`, `.cpt`, `.dtshd`, with or without core) and TrueHD / MLP streams frame by frame without loading them (the 4.7 GB DTSHDMASE Oppenheimer file indexes in about 1.5 s), and offers:
  - **Info** (format, rate, channels, length, size, average/peak kbps, restart points; recognises X96 and XCH),
  - **Bit rate over time** chart (mean and peak, replaces the PBR graph of the old tool),
  - **Trim** (frame-accurate cut points; TrueHD/MLP start at the nearest earlier restart point),
  - **Split** at any number of times into numbered files,
  - **Append** (checks format and sample rate match),
  - **Verify** (decodes everything with FFmpeg, reports decoder complaints and the MD5 of the decoded PCM).
  Container headers (`.dtshd`) are patched (frame count, sample count, data size); splitting the 7.1 test file at 20 s and 40 s and appending the three parts reproduces the original file byte for byte; a trimmed `.dtshd` passes both the Verify tool and DTSHDVerify.
  - Not done from the old tool's StreamTools: Join/Replace, Re-stripe, Add Silence.
- GUI fixes found while testing: the `hidden` attribute lost against component display rules (the MA core checkbox and DTS options showed for AC-3) - now global; the new DTS formats were still listed as "(planned)" and routed to FFmpeg - the catalog now knows which streams FFmpeg can make and the app uses the native encoder for the rest. End-to-end through the app queue with auto-verify: DTS-HD HRA, DTS-ES and MLP jobs all pass.
- **Speed (partial).** The 64-band extension analysis and the 96 kHz-to-48 kHz decimation of the 96/24 and 96 kHz MA encoders now run one thread per channel (96/24 5.1: 10.8 s -> 6.9 s for 12 s of audio; 96 kHz MA 5.1 about 1.5x real time with the core included); output is byte-identical. The rate-distortion search of XBR/X96 prunes dominated candidates. The 48 kHz MA path was already about 4.8x real time. Remaining hot spots: the exact-decoder emulation in the core stage and X96 side-information coding.
- **MA 6.0 / 6.1 / 7.0** added to the hierarchical layout code (Cs folds into both surrounds at -3 dB; no LFE is fine): lossless with core in the new tests.
- **Presets (done).** Settings dialog: Save preset / Load preset (`.swpreset`, plain JSON: destination, stream type, layout, sample rate, bit rate, MA core on/off, dialnorm, DTS options, unlocked, output name/folder; input files are not part of it). Round-trip checked in the running app.

## DTS-HD Express (LBR) — first working encoder
- New native LBR encoder (`lbr*.rs`): hybrid MDCT filter bank, VLC-coded residuals, grid scale factors, stereo pairs. Mono, stereo and 5.0 (L R Ls Rs C) streams decode in FFmpeg; written as `.dtshd` extension-substream frames.
- Wired into the CLI (`--stream dts-express`), the queue and the app catalog (48-510 kbps, default 128).
- Measured SNR on dialog: mono 64/128/256 kbps = 11/15/21 dB; 5.0 at 384 kbps about 12 dB per channel.
- Known gaps: no LFE channel (5.1 layouts are rejected), no band-limit flags, so 64 kbps mono overshoots (about 76 kbps), and the parameter search is slow (about 1x real time for stereo, 0.3x for 5.0). Quality is modest compared with the reference encoder.

## DTS Express: LFE, 5.1, much faster
- LFE channel (`lbr_lfe.rs`): 750 Hz decimation plus 4-bit ADPCM, as the decoder expects; a 60 Hz test tone decodes at 0.48 amplitude (input 0.5) in the correct output channel. 5.1 now works (City of God 10:00, 384 kbps: 386 kbps average, FFmpeg shows "DTS Express 5.1(side)").
- Quantiser search is now a binary search: 5.1 encodes at 8x real time (was 0.3x for 5.0).
- Encoder buffers are drained as frames are produced, so long programmes no longer grow in memory.
- Still not done: band-limit flags (the 32-band layout is fixed, so very low rates overshoot: mono 64 kbps averages about 76 kbps).

## NHK 22.2 (24 channels)
- New layout `22.2` (C L R Lw Rw Lss Rss Lb Rb Cb LFE LFE2 Tfc Tfl Tfr Tsl Tsr Tc Tbl Tbr Tbc Bfc Bfl Bfr) and the 12 new channel positions (LFE2, top layer, bottom layer), with file-name IDs including `Lb Rb Cb LFE2`.
- Auto-import now ignores per-channel numbering such as `ch01`, so dropping `...ch01 C.wav` finds the other 23 files of the Tokyo 2020 set.
- Mix-down of any richer input layout to any output layout by constant-power panning between the two nearest speakers (elevated or lowered sources -3 dB, overhead spread over all speakers, the two LFEs summed at -3 dB each). Implemented as `Matrix::pan` and applied while the stems are read (`StemSet::set_downmix`), so a 4-hour film is never written out as a temporary file.
- CLI: `--source-layout 22.2 --layout 5.1 ...`; queue: `EncodeSpec.source_layout`; GUI: "Input layout" selector over the file table. Native encoders only (every format now has one).
- Checked on Tokyo 2020 (1:00:00-1:00:30): 22.2 to 7.1 DTS-HD MA decodes in FFmpeg with sensible per-channel levels; 22.2 to 5.1 DTS Express encodes.
- Not yet: a 22.2 output format (there is no public 24-channel codec spec in the roadmap; multichannel WAV/FLAC pass-through is a candidate), and a downmix editor for the 22.2 matrix.
- Also fixed: the queue test that expected the native engine to refuse MLP (MLP became native earlier).

## AC-3 block switching (transient handling)
- `blocksw.rs`: the spec's 256-sample transform pair (clause 7.9.4.2) written literally as a reference inverse, plus the forward transform as its adjoint. Tests: the long block matches the fast MDCT, and any mixed sequence of long and short blocks reconstructs perfectly (error 4e-7) through the spec decoder.
- Transient detector as in clause 8.2.2 (8 kHz high-pass, three-level segment peak tree, thresholds 0.1/0.075/0.05, silence gate). Channels switch independently per block.
- A switched block gets exponent sets of its own using D45 (as the spec recommends); neighbouring blocks no longer share exponents with it.
- FFmpeg decodes the result with CRC checking on; on a synthetic clap signal at 96 kbps the error energy before each onset (pre-echo) drops 2.5 dB. `SW_AC3_NOBLKSW=1` turns it off for A/B tests; E-AC-3 stays on long blocks.
- Not done: coupling and rematrixing (still the main efficiency gap at low rates).

## AC-3 rematrixing (2/0 mode)
- Per block and per band (the four bands of Table 7.25) the encoder sends sum/difference when it leaves less energy to code, using the spec's "minimum sum" rule; flags are only resent when they change; blocks whose two channels differ in block switching are never rematrixed.
- Joint-stereo test material at 96 kbps: 19.2 dB with plain L/R, 21.0 dB rematrixed (decoded by FFmpeg).
- `SW_AC3_NOREMAT=1` switches it off for A/B tests. Coupling is still not implemented.

## StreamTools: Replace and Insert
- `subwoofa tools replace BASE WITH --at SECS -o OUT` swaps the stretch of BASE that WITH covers for WITH (the Join/Replace tool); `tools insert` adds WITH at a point without removing anything. Both work on DTS, DTS-HD, TrueHD and MLP, copy bytes untouched, snap cuts to frames decoding can start at (restart points for TrueHD/MLP) and patch the `.dtshd` header totals.
- GUI: new Replace tab in StreamTools (replacement stream, time, "Insert only").
- Checked on a 30 s DTS-HD MA 7.1 stream: replacing 12 s onward with a 4 s cut of itself gives the same length, 2813 restart points and a clean decode in the verifier.
- Still not implemented: Re-stripe and Add Silence (generating silence needs an encoder configured exactly like the source stream).

## DTS Express now passes DTS's own verifier
- The first Express streams decoded in FFmpeg but failed `DTSHDVerify.exe` at frame 0. Causes found by experiment: the extension-substream descriptor must say the LBR sync word is present (and give a sync distance of 0); frames now carry the checksummed frame chunk (id 0x04); and the `.dtshd` header now says "LBR encoding, no core, constant bit rate" instead of copying the lossless template.
- Result: mono, stereo, 3.0, 4.0, 2.1/3.1/4.1, 5.0 and 5.1 streams verify with every frame accepted.
- Constant frame size is enforced. A channel pair cannot be coded below the full mono region (the decoder reads those band indices in its second pass), so the cheapest setting costs about 108 kbps per pair, 36-48 kbps for a lone channel and 3 kbps for the LFE. The encoder now refuses lower rates with a clear message, the format catalogue shows the real minimum per layout (for example 5.1: 255 to 510 kbps), and if a loud frame still does not fit, the cheapest legal setting with muted coefficients is written instead of an oversize frame.
- Still open for Express: band-limit flags (would allow lower rates), tonal components, LFE at 24 bits.

## Speed
- Per-frame `thread::scope` spawns (tens of thousands of thread creations per minute of audio) replaced by a persistent rayon pool in the core analysis, MA core input, XLL channel planning, TrueHD channel analysis, 96/24 core input and extension analysis, and in the XBR and X96 candidate searches.
- MA lossless stage now runs one batch behind the core stage (pipelined) instead of blocking it.
- The core's quality-offset search evaluates candidates in parallel on machines with 6+ cores (same 0.375 dB resolution; output unchanged).
- 60 s of 5.1 film audio: MA 3.7x -> 7.6x real time, HRA 4x -> 7x, DTS core 7x -> 11x; 40 s of 96 kHz 5.1 in 96/24: 1x -> 3.7x. Output files are bit-identical to before for HRA and 96/24.
- Auto-import also understands FFmpeg-style names (`ch1 FL_Front-Left`: `FL FR FC SL SR BL BR` and descriptive words such as Front/Left/Center are ignored when matching siblings).

## DTS Music Disc (DTS-CD) output was a raw stream named .wav; now a real DTS-in-PCM WAV
- Destination "DTS Music Disc" now writes a 16-bit stereo PCM WAV (44.1 kHz, or 48 kHz) whose samples are the stream's byte-swapped 16-bit words, with the CD data rate of exactly 1411.2 kbit/s (2048-byte core frames). FFmpeg identifies the file as "dts, 44100 Hz, 5.1, 1411 kb/s" and decodes it cleanly.
- Sample rates are now per destination: DTS Music Disc 44.1 kHz, video discs/delivery 48 kHz for core DTS and ES (96 kHz for 96/24), and the GUI's rate list follows the destination. Bit-rate range for a music disc is 754 to 1411 kbps.
