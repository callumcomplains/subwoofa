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

## Folder-based batch encode
- New "Batch from folder…" button over the input table: pick a folder and every complete set of per-channel files for the chosen input layout (`Name_L.wav`, `Name_R.wav`, … also `chNN`/FFmpeg-style names) becomes one queued job with the current format settings, named after the programme. `autoimport::scan_folder` does the grouping (incomplete sets are skipped; unit tested); Tauri command `scan_folder`.

## E-AC-3 7.1 / 7.0 (dependent substream)
- The catalogue offered 7.1 for E-AC-3 but the encoder refused it. Now: an independent substream carries a 5.1 (5.0) downmix (rears folded into the surrounds at -3 dB), and a dependent substream (strmtyp 1, custom channel map: Ls, Rs and the Lrs/Rrs pair) carries the true side and rear channels, so 7.1 decoders replace the downmixed surrounds and add the rears. The bit rate is split by channel count.
- Found and fixed along the way: dependent substreams have no `convexpstr` fields and no per-block `convsnroffste` (A/52B Annex E); FFmpeg reported "expacc out of range" until they were left out.
- Test: eight different tones, one per 7.1 channel, come back from FFmpeg each in its own output channel with no leakage. Also: side-surround file names (`SL`, `Ls`) now match `Lss` layouts in auto-import.
- Layouts accepted for E-AC-3 are now the AC-3 modes plus 7.1 (side+rear, or surround+rear) and 7.0.

## DTS Express: band limiting, much lower rates
- The decoder's band-limit modes (1/2 and 1/4 of the bandwidth: 16 or 8 coded subbands) are now used. They shrink every grid and make a channel pair's mandatory mono region cheap, which was what held low rates back. The mode follows the rate per full-band channel: 128 kbps and up full band (24 kHz), 56 to 128 kbps 12 kHz, below that 6 kHz. Streams verify with `DTSHDVerify` and decode in FFmpeg in all three modes.
- Mono at 64 kbps: SNR 5 dB -> 16 dB (the old full-band setting spread too few bits over 32 bands; now the bits land in the 0-12 kHz range where the speech is). 96 kbps: 13.7 -> 20.2 dB.
- Minimum rates are now about 56 kbps per channel pair, 30 kbps for a lone channel and 3 kbps for the LFE (5.1 from 143 kbps instead of 255; stereo from 56 instead of 109). The catalogue and the encoder's own check use the same numbers.
- Bug fixed: stream frames must have an even size (the verifier rejects odd frames, FFmpeg did not mind); for example 128 kbps stereo was 1365 bytes per frame and failed.
- Bug fixed (MA, all full-length encodes): `ma_core.rs` compared `keep_from > self.dropped + 1 << 16`, which Rust reads as `(dropped + 1) << 16`, so the input buffer was never trimmed after the first frame and memory grew with the length of the programme (about 11 MB/s of audio; a 2 h 7.1 film would have needed more than 11 GB). Fixed; memory stays flat.

## DTS-HD HRA 7.1 / 7.0 (XXCH extension)
- Reference: a real HRA 7.1 stream (2,046 kbps) from a commercial disc, supplied by you. Its structure: a 5.1 core at 1509 kbps whose surround channels are the side pair with the rears folded in at -3 dB (every core channel pre-scaled by 0.707), plus an XXCH extension in the extension substream coding the rear pair, with an embedded-downmix header so decoders can undo the fold.
- New: native HRA for 7.1 side+rear, 7.1 surround+rear and 7.0 layouts builds exactly that: core 5.1 (up to 1509 kbps, the rest of the rate to XXCH), `XxchParams`, `Encoder::encode_xxch`, EXSS asset with the XXCH component, `.dtshd` container. Everything matches the reference stream field for field (core frame 2012 bytes, EXSS 716 bytes, extension mask 0x41, speaker mask 0x84B, header sizes, downmix scale and coefficient codes).
- Checks: DTS's verifier accepts all frames at 2046, 3018 and 5376 kbps, 7.1 and 7.0; FFmpeg decodes 7.1 with the downmix undone. A full test (eight different tones, one per channel) returns every channel in the right place with no leakage; re-encoding the commercial stream's own audio gives 47 to 58 dB SNR per channel (double-lossy, so this measures our coding only).
- Found while testing with loud rear channels: the XXCH downmix coefficient code is sign-and-magnitude (bit 6 set = positive); getting the sign wrong left the sides carrying the rears.
- LFE: the encoder's LFE decimation filter reused the decoder's own interpolation prototype, so the LFE was attenuated twice (-4 dB at 100 Hz, overall gain 0.79). It is now designed as the inverse of the decoder's response up to 110 Hz: gain 0.93, 80-120 Hz within 2 dB.

## HRA 7.1 at higher rates: XBR + XXCH
- A commercial 5376 kbps HRA 7.1 stream (from your library scan) uses both extensions: XBR for the five core channels (3280 bytes per frame) and XXCH for the pair (1844). The native 7.1 encoder now does the same: the extra pair first gets the core's per-channel rate, and everything beyond that is shared between XBR and XXCH in the ratio of their channel counts. Verified at 2046, 3018 and 5376 kbps (verifier and FFmpeg); at 5376 kbps the decoded channels are 87 to 100 dB SNR against their source.
- Library scan results, noted for planning: the commercial DTS 96/24 clips fail DTS's verifier at frame 2 (our own 96/24 output passes it), HRA 5.1 and 7.1 files have a 28-byte extension header where ours has 27 (harmless).

## Reference streams from your files: DTS-ES, DTS-CD
- **DTS-ES discrete:** Hero, LOTR 1, 2 and 3 are all ES discrete (5.1 core at 1509 kbps with the sixth channel in an XCH extension at the end of each 2012-byte frame; the XCH share follows the content: 207 to 332 bytes). All four verify with DTS's verifier on every frame, and my ES discrete output has the same header flags and layout. The only difference is that their frames set the "sync after every sub-subframe" flag.
- **DTS-CD (The Wall .nrg):** the Nero image is plain 2352-byte sectors; the tracks are DTS in 14-bit packing: 14-bit words sign-extended into 16-bit little-endian samples, 1024-sample frames of 3584 stream bytes (4096 bytes on the disc), sync every 4096 bytes, core rate code 22 (the disc's 1411.2 kbit/s label), 1234.8 kbit/s of actual stream. My earlier DTS Music Disc wrapper wrote 16-bit packing with 512-sample frames. It now writes the real thing: first bytes identical to the commercial disc, DTS's verifier reports core bit rate 1234 and accepts every frame, FFmpeg decodes it clean.
- Also fixed: auto-import with a bare file name (no folder part) found no sibling files.

## DTS-HD MA with a 96/24 core (X96) — done
- What the library scan found: several of the 96 kHz MA tracks on the Demo Discs (Brothers in Arms, Walk of Life, Lady Madonna, Postcard...) carry a 96/24 core: the 48 kHz core frames have an X96 extension at their tail (about 256 of 2012 bytes) and the extension substream asset says so with mask 0x205 instead of 0x201. They verify cleanly with DTS's verifier.
- The reference decoder's design (FFmpeg's comment: "a special case for lossless residual decoder that ignores X96 data if present") explains how it fits together: the lossless residual is relative to the core upsampled by the 64-band synthesis, and the X96 data is only for decoders that play the core substream alone. So my earlier worry (no way to verify) was wrong: the existing MA residual stays exactly as it was and the X96 block is simply added to the core.
- Built: `--core-96-24` (CLI), `core_x96` (queue), "96/24 core" checkbox (app, shown for MA at 96 kHz with a core). The core gets an X96 reserve, the programme is analysed with the 64-band bank aligned to the core's subbands, the lower half minus what the core coded plus the upper half go into the X96 block, and the EXSS descriptor sets the extension bit.
- Checks (Dire Straits 5.1, 20 s): DTS's verifier accepts all 1875 frames; FFmpeg decodes the full stream bit-exactly (all six channels identical to the source); stripping the extension substream leaves a stream FFmpeg identifies as "DTS 96/24, 96000 Hz" at 41 to 46 dB SNR, against 25 dB for the same core without X96. The cost is 2.5% on file size (16313 vs 16725 kbps on this loud live material). Regression test added (96 kHz 5.1, lossless through FFmpeg).

## AC-3 channel coupling
- Multichannel AC-3 (3/0 and richer, below about 96 kbps per channel) now couples the channels above a start frequency that follows the rate (about 5.7 kHz at 192 kbps for 5.1, 10 kHz around 320 kbps): the coupled channels send one shared coupling channel (their mean) plus per-band coupling coordinates (energy ratio to the coupling channel, quantized to the format's 2-bit master / 4-bit exponent / 4-bit mantissa), refreshed with every set of blocks. The coupling channel has its own exponents (reference exponent halved and kept even, groups inside the +-2 step limit), masking curve with the leak start values, bit allocation and mantissas, in the spec's order (after the first coupled channel). Frames with block switching are not coupled. `SW_AC3_NOCPL=1` switches it off.
- Same-rate quality on City of God 5.1 (SNR against the stems, front channels): 192 kbps +1.6 dB, 320 kbps +2.2 dB, 384 kbps +2.4 dB; surrounds +0.3 to 0.4 dB. FFmpeg decodes with CRC checking and no complaint at 3.0, 4.0, 5.0 and 5.1 from 96 to 640 kbps.
- Bugs fixed on the way: the coupling reference exponent must be even (it is sent halved), and fixing it has to happen on the group sequence, not per bin.
- Not done: coupling phase flags (2/0 mode), band structure (every 12-bin sub-band is its own band), E-AC-3 enhanced coupling.

## Peak bit rate in every encode log
- When a job writes a DTS, DTS-HD, TrueHD or MLP stream, the job log now ends with the peak bit rate (single frame and busiest second, with their positions) read back from the finished file by StreamTools, so a disc-authoring limit can be checked without opening another tool. Also noted from the commercial `.dtshd` file MAS wrote: it contains two small metadata chunks (`CORESSMD` and `EXTSS_MD`) that mine lacks; its header's frame-rate field matches the log ("24") and its start-time field is not read by DTS's verifier, so StreamTools re-stripe stays unimplemented until those fields can be verified.

## DTS Express checked against a commercial stream (Source Code, secondary audio)

The user's Source Code `00004.m2ts` (truncated, but the Express track is intact) gave the first real DTS Express reference: stream 29, pid 0x1A00, stereo, 187,872 bit/s. `DTSHDVerify` passes it (712/712 frames), frames are a fixed 2048 bytes.

Compared field by field with our output and matched:
- Extension substream **index 2** (was 0); the presentation names substreams 0 and 2 (mask 0b101, one asset each) and carries mixing metadata (adjustment level 0, one 4-bit output config, mask 0xF).
- Asset descriptor: **24-bit** resolution, a mixing-metadata-present bit (0), "decode in secondary decoder" set, followed by seven fixed bits (`0100101`) of unpublished meaning. Copied verbatim.
- The LBR header now announces the **payload bit rate** (frame minus the 16-byte LBR header, rounded down to a multiple of 32), not the nominal rate. Flag bit 0 is set as in the reference.
- Frame chunk is followed by two zero bytes inside the frame.
- The bit rate now counts the **whole** extension substream frame, so a 192 kbps request gives exactly 2048-byte frames (it was 195 kbps average before: the header came on top).

Verified with DTSHDVerify and FFmpeg: 2.0 at 64/192/510, 1.0 at 48, 5.0 at 192, 5.1 at 160/256 all pass.
Not yet learned from the reference: how the commercial encoder splits its 188 kbps (band limit, tonal components); the header byte pattern after the flags is identical to ours at 1/2 band limit.
