# To do

Tick a box when the work is finished, tested and committed. Order within a section is the order of work.
Estimates are focused working time for the work itself.

## Now: the agreed order

- [x] **WavPack** (`.wv`) native lossless encoder, mono to 5.1+ (about 6 h). Checked by decoding with FFmpeg. (Done: size within 1% of FFmpeg's default encoder.)
- [x] **Monkey's Audio** (`.ape`) native lossless encoder (about 6 h). Checked by decoding with FFmpeg. (Done: mono and stereo, 16/24 bit.)
- [x] **MPEG-4 ALS** (`.als`) native lossless encoder (about 8 h). Checked by decoding with FFmpeg. (Done, in an MP4 wrapper for the check. Not done: block switching, long-term prediction, BGMC; they would take it from FLAC-sized to about 2% better.)
- [x] **DTS-HD Express: tonal components**: already done earlier (steady notes are coded as tones; this item was stale). Quality against the original was also measured and is at or above it at the same bit rate on most test cases.
- [x] **DTS-HD Express**, the follow-up round: the encoder is already below the original's own floors (mono 48, stereo 64, 5.1 146 kbps, against the original's 48 / 64 / 192), so no lower floor is needed; quality parity was measured earlier; the quantizer-profile search now runs on all cores (same bytes out, about 3x faster: 5.1 encodes at about 6x real time); the LFE uses the 6-bit coding from 256 kbps (checked with FFmpeg and DTSHDVerify).
- [x] Express, small leftovers, looked at: interpolating scale factors over time would save about 20 bytes of a 1024-byte frame at mono 96 kbps (about 0.1 dB), the attenuation curve was tried earlier and did not help, and the tone search depends on its own earlier choices, so it cannot run in parallel without changing the result. None is worth the risk.

Each format means: encoder in `sw-codec-pcm`, stream type and Files destination, CLI, GUI (plate, extension, pcm
format options), verify support, round-trip tests, docs table row, changelog line.

## Release follow-ups (small)

- [ ] Install the release on a clean machine: SmartScreen, version display, no-FFmpeg banner, a native encode.
- [ ] Turn on private vulnerability reporting (repository Settings > Advanced Security).
- [ ] Check the in-app "How" links open the right docs pages.
- [x] `CONTRIBUTING.md` (how to build, test, and the code standard below).
- [ ] Code signing for Windows, once there is a certificate.

## Next, unblocked

- [x] **Speed round** (the user's request: as close to FFmpeg as possible, even faster, trading file size if needed; options for cores and for fast/slow encodes, with a size warning). Done: frame- or interval-parallel encoding for AC-3/E-AC-3, DTS core/HRA/Master Audio and TrueHD (byte-identical output), exact rewrites of the DTS filter bank, the XLL covariance/autocorrelation, ALS analysis and the FLAC Rice search, parallel interleave and read-ahead input, mimalloc, fat LTO; the `Effort` setting (fast/balanced/best) for TrueHD, DTS-HD MA, FLAC, ALAC and ALS, measured on a corpus (`subwoofa-lab/effort_eval.py`); `--threads`; GUI speed control with the size note, Settings > Performance; `docs/SPEED.md`.
- [ ] Speed, left: DTS-HD Express stays at about 5x real time (22 s for two minutes of 5.1): a frame's tone choice and its rate search feed each other, so frames cannot be encoded side by side; the pairs, profiles and tone candidates within a frame already run in parallel. Single-core AC-3/TrueHD stay 3 to 4 times slower than FFmpeg's, and FLAC takes 1.0 s against FFmpeg's 0.6 s (the floor is mostly converting and cutting blocks). Done in the second speed round: the 7.1 ES/XXCH path (9 s to 4 s) and the 96 kHz High Resolution path (10.7 s to 5.1 s for 5.1), byte for byte the same.
- [x] **Readability pass**, first round: the whole code base is `rustfmt`-formatted at 120 columns and CI checks it; the dead-code warnings are gone; every file has a module comment (five were added, three stale ones corrected); `docs/ARCHITECTURE.md` maps the crates and says how to add a format. Second round (done): every file over about 900 lines was split along the stages of the format it handles (TrueHD, AC-3, the DTS family, the verifiers, StreamTools, the PCM reader, the Atmos backend, the command line, the app bridge and the page script), the longest functions were cut into named steps, `clippy` is clean and runs in CI, and every change was checked to leave the output files byte for byte the same (`crates/sw-queue/tests/golden.rs` holds a checksum for each format). Left: functions of 100 to 150 lines in the oldest DTS and TrueHD code, and a few longer names that could be shorter.
- [x] **DCP audio**: done (SMPTE and Interop, checked against asdcplib's own output and tools; not tried on a cinema server). Encrypted track files and the rest of a DCP (CPL, PKL, ASSETMAP) are not made.
- [x] **AC-3 exponent-strategy optimisation**: measured instead of built. Eight combinations of exponent strategy and block grouping were tried on film audio (stereo 192 and 384, 5.1 at 448 and 640 kbps, decoded SNR): all within 0.2 dB of the current choice, so the fixed choice stays.
- [x] ALS extras, tried: block switching is in (16-bit split field, up to 16 blocks per frame; 0.1% to 0.3% smaller on film and music, about twice the encode time). Predictor orders up to 512 gained 0.02%, so the limit stays at 64. Long-term prediction and the BGMC coder were left out: on this material the gains measured so far were below 0.5%, and BGMC needs the format's arithmetic-coder tables.
- [x] GUI leftovers: checked against the old encoder's list: presets, folder batch, automatic verify and the peak bit rate in the job log all exist, and a source file's BWF time reference is carried into WAV output. Not there: a field to type a BWF start time by hand.

## Roadmap: making it more useful (ranked)

1. [x] **Loudness built in.** `sw-loudness` (BS.1770-4: integrated, short-term, momentary, loudness range, true peak; checked against FFmpeg's ebur128 on a 5.1 programme: same integrated loudness and range), `subwoofa loudness`, `--auto-dialnorm`, a Measure button beside Dialnorm in the app (it flags a true peak over -1 dBTP; the downmix clip check was already there). Normalising to a target (`--normalize`, a row in the app) is done too.
2. [x] **A real QC report.** `subwoofa verify --report out.html` and the app's Save report (checks, loudness, per-channel peaks, bit rate chart, per-channel spectrograms), and `subwoofa verify FILE --against SOURCE` (decode lined up with the source by cross-correlation, channels matched by content, bit-exact test for lossless, signal-to-noise per channel for lossy).
3. [x] **Preview before committing.** A Preview button beside Encode: encodes 20 seconds from the middle of the programme with the job's exact settings, decodes it, and plays the source, the encode and the difference between them in sync with an instant switch (keys 1, 2, 3, space). Reports the bit rate and signal to noise of the excerpt. Excerpt choice and a blind A/B mode are done. Left: a multichannel monitor instead of the stereo fold-down, a command-line `subwoofa preview` that writes the three WAV files.
4. [ ] **Pipeline and automation.** Done: JSON job files (`encode --save-job`, `run`) and a watch folder (`watch`). The app has a Watch folder button too. Left: DAW stem-naming conventions in auto-import, a headless encode server.
5. [ ] **More inputs.** A video file or an existing DTS / AC-3 / TrueHD stream as input (with a warning for lossy to lossy), frame-rate conversion (23.976, 24, 25 fps, with and without pitch change), an audio delay control (done: `--delay-ms`).
6. [ ] **Metadata edits without re-encoding.** Done: dialnorm of AC-3 / E-AC-3 streams (`tools dialnorm`, StreamTools > Dialnorm). Left: DRC profile, DTS and TrueHD header fields.
7. [ ] **Better downmix design.** Presets from ITU-R BS.775 and the Lo/Ro, Lt/Rt conventions, loudness-matched stereo, a picture of what each speaker feeds.
8. [ ] **Reach.** macOS and Linux builds, winget / scoop packages, a "first disc in ten minutes" tutorial.

## Blocked on outside files or tools

- [ ] TrueHD Atmos: run on one real Dolby-authored master (built, never run on one).
- [x] More of the Dolby Encoding Engine wired up: dialogue-aware loudness, Dolby Digital / Digital Plus encode and decode, transcode, trim and join, loudness correction. Not available in the installed build: AAC, AC-4, and channel reordering on short content.
- [ ] Bitstream tab extras: the reference encodes in `docs/REFERENCE-ENCODES.md`.
- [ ] HRA 6.1 at 48 kHz (XCH + XBR): needs its own reference encode.
- [ ] DVD-Audio disc structure (AOB packs): one real DVD-Audio disc or its `.AOB` files.
- [ ] Dolby Atmos Theatrical (IAB in MXF): one real cinema track file, after DCP audio.
- [ ] DTS:X encoding: DTS's licensed encoder.
- [ ] AAF import: AAF files from a DAW.
- [ ] E-AC-3 enhanced coupling: the reference decoder now exists (the Dolby Encoding Engine's `ddp_decode`, see `subwoofa dolby decode`), so this can be built and checked. Subwoofa's current streams decode alike in Dolby's decoder and FFmpeg (AC-3 agree to about 66 dB, E-AC-3 to about 40 dB, and both are the same distance from the source).

## Parked (no realistic route yet)

AC-4 (the Dolby Encoding Engine cannot encode it), MPEG-H 3D Audio, Auro-3D, Datasat, DVD-Audio/SACD/DCP
hardware testing.

## Code standard for everything new

New code is written so it never needs a readability rewrite:

1. **Say what and why.** Every file starts with a module comment: what it is for, which specification it follows
   (with the section or table names), and the order the pieces run in.
2. **Names are short words.** `step`, `window`, `fold_sign`: not `qs`, and not a sentence either. Two or three words at most, test names included (`parameter_tracks_level`, not a description of the whole behaviour). The spec's abbreviation may appear once in a comment.
3. **Short functions that do one thing**, ideally under about 40 lines, lines under about 110 characters.
4. **Constants and tables are named and sourced.** No bare magic numbers; each table says where it comes from.
5. **Comments explain why**, not what the next line does.
6. **Each piece has a test** that fails when the piece is wrong, and the round trip is verified by an independent
   decoder where one exists.
7. **No dead code, no debug leftovers.**
