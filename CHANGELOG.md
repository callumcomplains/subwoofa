# Changelog

Versions are shown as `b<version> (<UTC build time>)`. The `b` means public beta and goes away at 1.0.

## Unreleased

- New audio-file formats, all written natively and checked by decoding with FFmpeg: **WavPack** (`.wv`), **Monkey's Audio** (`.ape`, mono and stereo) and **MPEG-4 ALS** (`.als`, or an MP4 file when the name ends in `.mp4`). The Apple Lossless writer now shares its MP4 code with ALS.
- **DCP sound**: an AS-DCP / SMPTE ST 429-3 track file (`.mxf`) of 24-bit PCM at 48 or 96 kHz, up to 16 channels, SMPTE or MXF Interop. Matches the reference toolkit's header byte for byte apart from identifiers; not tried on a cinema server.
- Verify understands all of them.
- DTS Express encodes about 3x faster (profile search runs on all cores), uses the 6-bit LFE coding from 256 kbps.
- `CONTRIBUTING.md` added.

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
