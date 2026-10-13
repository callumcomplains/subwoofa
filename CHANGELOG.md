# Changelog

Versions are shown as `b<version> (<UTC build time>)`. The `b` means public beta and goes away at 1.0.

## Unreleased

- New audio-file formats, all written natively and checked by decoding with FFmpeg: **WavPack** (`.wv`), **Monkey's Audio** (`.ape`, mono and stereo) and **MPEG-4 ALS** (`.als`, or an MP4 file when the name ends in `.mp4`). The Apple Lossless writer now shares its MP4 code with ALS.
- **DCP sound**: an AS-DCP / SMPTE ST 429-3 track file (`.mxf`) of 24-bit PCM at 48 or 96 kHz, up to 16 channels, SMPTE or MXF Interop. Matches the reference toolkit's header byte for byte apart from identifiers; not tried on a cinema server.
- Verify understands all of them.
- DTS Express encodes about 3x faster (profile search runs on all cores), uses the 6-bit LFE coding from 256 kbps.
- `CONTRIBUTING.md` added.
- **Much faster encoding.** Every encoder now spreads its work over all cores: AC-3 and E-AC-3 (2.2 s to 0.65 s for two minutes of 5.1), the DTS family (core 12.6 s to 2.4 s, High Resolution 20 s to 4.7 s, Master Audio 19 s to 5.6 s), TrueHD (12.4 s to 1.7 s) and the audio-file formats (FLAC, Apple Lossless, MPEG-4 ALS, WavPack, TTA, Monkey's Audio, WAV). The files are byte for byte what the slower version made. Reading input overlaps with encoding, and a post-encode scan that cost more than the encode of a WAV file is gone.
- **Speed setting for lossless formats** (`--speed fast|balanced|best`; a control next to the Encode button): Balanced is the default and makes files up to 1% larger than Best for TrueHD, DTS-HD Master Audio, FLAC, Apple Lossless and MPEG-4 ALS, several times quicker; Fast trades a few per cent more. The app says what each costs for the chosen format. Best is what earlier versions made.
- **Cores setting** (`--threads N`; Settings, Performance): limit an encode to a number of cores, or use all of them. The output is the same either way.
- Faster release build (link-time optimisation), a faster memory allocator, and the DTS filter bank, Master Audio predictor search and ALS/FLAC analysis rewritten to give the same results in a fraction of the time.
- `docs/SPEED.md` rewritten with the new numbers, the cost of each speed setting and a note on one core against all of them.

## b0.1.0 — first public beta

**Encoders (all written from public specifications, no third-party code)**
- DTS: Digital Surround core, ES (Matrix and Discrete), 96/24, HD High Resolution, HD Master Audio (lossless, up to
  7.1 and immersive layouts up to 22.2), HD Express.
- Dolby: AC-3, E-AC-3, TrueHD, MLP (DVD-Audio).
- Audio files: WAV/BWF/RF64, AIFF, CAF, FLAC (multichannel, also in Ogg), ALAC, TTA, DSD (DSF and DSDIFF).
- Dolby Atmos through a Dolby Encoding Engine you supply.

**Features**
- 30 channel layouts, Blu-ray / DVD / DTS Music Disc / digital-delivery rules, unlocked mode for experts.
- Bitstream tab (timecode, seamless branching, commentary levels) and Downmix tab (embedded stereo, 7.x to 5.1).
- StreamTools (info, bit-rate chart, trim, split, append, replace, re-stripe, silence) and Verify (simple and deep).
- Encode queue with progress, cancel, concurrency and automatic verification.
- Ten themes and a custom theme; presets; automatic file names such as `SUBWOOFA - BD DTSHDMA 7.1 96kHz 24FPS`.
- Settings page with custom paths for FFmpeg, the DTS-HD Master Audio Suite and the Dolby Encoding Engine. When a
  tool is missing, the features that need it are switched off and the app says how to fix it.
- Version shown in the app and printed by every command-line run.

**Known limits**
- Windows only for now; macOS and Linux are untested.
- Release files are unsigned (SmartScreen will warn).
- TrueHD Atmos is built but not yet run on a real master. DTS-HD Express quality is modest.
