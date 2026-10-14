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
- **Loudness**: `subwoofa loudness` measures integrated, short-term and momentary loudness, loudness range and true peak (ITU-R BS.1770-4 / EBU R 128; agrees with FFmpeg's `ebur128` filter), `subwoofa encode --auto-dialnorm` sets the dialnorm from the integrated loudness, and the app has a Measure button beside Dialnorm.
- **QC report**: `subwoofa verify FILE --report out.html` (and Save report in the Verify tab after a deep check) writes one self-contained web page: the verdict, loudness and true peaks, every check, the bit rate over time and a spectrogram of each channel. It prints cleanly, so it can go with a delivery.
- **Compare with the source**: `subwoofa verify FILE --against SOURCE` lines the decode up with the audio it was made from, then proves a lossless file identical sample for sample, or gives a signal-to-noise figure per channel for a lossy one. The result goes into the QC report.
- **Preview** (the Preview button beside Encode): encodes 20 seconds from the middle of the programme with the job's own settings, decodes it again and plays the source, the result and the difference between them, lined up and level-matched, so you can switch between them while it plays. It also reports the excerpt's bit rate and signal to noise.
- The Verify tab can compare a deep check with the source (Compare with source, with its layout and an optional start and length), and the installer's FFmpeg step saves a log you can open and copy.
- **Change the dialnorm of a finished AC-3 or E-AC-3 stream without re-encoding** (`subwoofa tools dialnorm FILE --set N`, and a Dialnorm tab in StreamTools): only the level and the checksums are rewritten, so the audio is bit for bit unchanged.
- **Normalise** (`--normalize LUFS`; a row under Dialnorm in the app): applies gain so the integrated loudness lands on a target such as -23 (EBU R 128) or -24 (ATSC A/85), keeping the true peak under -1 dBTP, which can leave a very peaky mix a little quieter than asked. Works with every format Subwoofa encodes itself.
- **Automation**: `subwoofa encode --save-job FILE` saves a job as JSON, `subwoofa run FILE` runs job files (one job, a list, or several), and `subwoofa watch FOLDER --job FILE --out DIR` is a hot folder that encodes every complete set of channel files that lands in it, safely (waits for copies to finish, never leaves a half file that looks done).
- Smaller programs: release builds are stripped of debug symbols (the app is about 9 MB, the command-line program about 4 MB).
- **Installer** restyled and extended: Subwoofa artwork, the licence page, a Start menu folder, and an **Extras** page with two optional checkboxes: add Subwoofa to your PATH (so `subwoofa` runs from any folder; the command-line program is now installed too, and uninstalling removes the entry) and download FFmpeg (checked against its published checksum, kept in Subwoofa's own folder; also a Download button in Settings and on the missing-FFmpeg banner). The WebView2 setup program is bundled so it need not be fetched first.
- DTS-HD High Resolution is faster for 7.1 (9 s to 4 s for a minute) and at 96 kHz (10.7 s to 5.1 s for 5.1), and DTS Express a little faster; the files are byte for byte what they were.
- Fixed: **Verify (deep)** of an E-AC-3 stream with dependent substreams (such as 7.1) stopped with an error while building the bit rate chart.
- **Code reorganised for reading** (no change to any output file: a checksum test covers every format, and the files before and after were compared byte for byte). Large files are split along the stages of each format, long functions are cut into named steps, `clippy` runs clean in CI, and `docs/ARCHITECTURE.md` maps the new layout. The command line's `encode` now goes through the same job runner as the app, so both give the same result for the same settings.

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
