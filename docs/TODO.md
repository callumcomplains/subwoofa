# To do

Tick a box when the work is finished, tested and committed. Order within a section is the order of work.
Estimates are focused working time for the work itself.

## Now: the agreed order

- [x] **WavPack** (`.wv`) native lossless encoder, mono to 5.1+ (about 6 h). Checked by decoding with FFmpeg. (Done: size within 1% of FFmpeg's default encoder.)
- [x] **Monkey's Audio** (`.ape`) native lossless encoder (about 6 h). Checked by decoding with FFmpeg. (Done: mono and stereo, 16/24 bit.)
- [x] **MPEG-4 ALS** (`.als`) native lossless encoder (about 8 h). Checked by decoding with FFmpeg. (Done, in an MP4 wrapper for the check. Not done: block switching, long-term prediction, BGMC; they would take it from FLAC-sized to about 2% better.)
- [x] **DTS-HD Express: tonal components**: already done earlier (steady notes are coded as tones; this item was stale). Quality against the original was also measured and is at or above it at the same bit rate on most test cases.
- [x] **DTS-HD Express**, the follow-up round: the encoder is already below the original's own floors (mono 48, stereo 64, 5.1 146 kbps, against the original's 48 / 64 / 192), so no lower floor is needed; quality parity was measured earlier; the quantizer-profile search now runs on all cores (same bytes out, about 3x faster: 5.1 encodes at about 6x real time); the LFE uses the 6-bit coding from 256 kbps (checked with FFmpeg and DTSHDVerify).
- [ ] Express, small leftovers: interpolated scale factors and the per-group attenuation curve (the last 1.6 dB at mono 96 kbps), parallel tone search.

Each format means: encoder in `sw-codec-pcm`, stream type and Files destination, CLI, GUI (plate, extension, pcm
format options), verify support, round-trip tests, docs table row, changelog line.

## Release follow-ups (small)

- [ ] Install the release on a clean machine: SmartScreen, version display, no-FFmpeg banner, a native encode.
- [ ] Turn on private vulnerability reporting (repository Settings > Advanced Security).
- [ ] Check the in-app "How" links open the right docs pages.
- [x] `CONTRIBUTING.md` (how to build, test, and the code standard below).
- [ ] Code signing for Windows, once there is a certificate.

## Next, unblocked

- [ ] **Readability pass** over the older code: per-crate guides, module comments on each pipeline, clearer names,
      shorter lines. Behaviour must not change; existing tests are the proof.
- [x] **DCP audio**: done (SMPTE and Interop, checked against asdcplib's own output and tools; not tried on a cinema server). Encrypted track files and the rest of a DCP (CPL, PKL, ASSETMAP) are not made.
- [x] **AC-3 exponent-strategy optimisation**: measured instead of built. Eight combinations of exponent strategy and block grouping were tried on film audio (stereo 192 and 384, 5.1 at 448 and 640 kbps, decoded SNR): all within 0.2 dB of the current choice, so the fixed choice stays.
- [ ] ALS extras: block switching, long-term prediction and the BGMC coder (about 2% smaller files).
- [ ] GUI leftovers: `.swsettings` polish, BWF timecode fields, folder-batch tweaks.

## Blocked on outside files or tools

- [ ] TrueHD Atmos: run on one real Dolby-authored master (built, never run on one).
- [ ] Bitstream tab extras: the reference encodes in `docs/REFERENCE-ENCODES.md`.
- [ ] HRA 6.1 at 48 kHz (XCH + XBR): needs its own reference encode.
- [ ] DVD-Audio disc structure (AOB packs): one real DVD-Audio disc or its `.AOB` files.
- [ ] Dolby Atmos Theatrical (IAB in MXF): one real cinema track file, after DCP audio.
- [ ] DTS:X encoding: DTS's licensed encoder.
- [ ] AAF import: AAF files from a DAW.
- [ ] E-AC-3 enhanced coupling: a reference decoder.

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
