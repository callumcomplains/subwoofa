# How Subwoofa fits together

A guide for people who want to read or change the code. Every source file starts with a comment saying what it is
for and which specification it follows; this page is the map between them.

## The shape of it

```
 stems (WAV, AIFF, ...)            the app (Tauri, app/ui)          the command line
        |                                   |                              |
        v                                   +---------- job spec ----------+
   sw-pcm: read as f32                                  |
        |                                               v
        |                                    sw-queue: run one job
        |                                     (log, progress, verify)
        v                                               |
   sw-codec-*: encode  <--------- sw-core: formats, layouts, limits
        |
        v
   .ac3 .thd .dtshd .flac .wv ...  --->  sw-verify: read it back, check it
```

A **job spec** (`sw_queue::run::EncodeSpec`) is plain data: the destination, stream type, layout, input files, bit rate,
rate, and every option. The app builds one from its controls, the command line from its flags, and a preset file is
one saved to disk. `sw-queue::run::execute` turns it into an encode and a log.

## The crates

| Crate | What it does |
|---|---|
| `sw-core` | The vocabulary: channel layouts and IDs (`layout`, `channel`), the capability matrix of destinations, stream types, rates and limits (`formats`), the Bitstream-tab settings (`bitstream`), downmix matrices (`downmix`), how hard a lossless encoder searches (`effort`), finding sibling channel files (`autoimport`), the version (`version`). |
| `sw-loudness` | Loudness as BS.1770-4 / EBU R 128 define it: `filter` (K-weighting), `meter` (blocks, gates, range, channel weights) and `peak` (true peak by oversampling). `sw-queue::measure` reads input files through it; the command line and the app's Measure button use that. |
| `sw-pcm` | Reading input audio: `reader` (WAV / RF64 / AIFF, streaming), `channels` (which file channel feeds which speaker) and `stems` (a set of per-channel files read together). |
| `sw-codec-ac3` | Dolby Digital and Digital Plus, in the order a frame is made: `analysis` (transform, block switching), `coupling` and `coupling_plan`, `exponents`, `bitalloc`, `mantissas`, `frame_plan` and `plan` (what to code), `syntax` (the bits), `layout` (channel mapping), `native` (the file driver). |
| `sw-codec-truehd` | TrueHD and MLP lossless encoder: `layout`, `downmix` (the stereo and 5.1 presentations), `filter` and `predict` (the FIR and IIR predictors), `coding` (Huffman and Rice), `substream`, `sync` (major sync and access units), `native` (the file driver), and `analyze`, which walks a stream the way a decoder would. |
| `sw-codec-dts` | The DTS family. `dca/` is the core (frame `encoder/`, filter banks, `native*` file drivers for the plain, 96/24 and 7.1 cases, `wav_out` for DTS in a WAV); `hra/` High Resolution (`xbr`, `stage`, `hra96`); `lossless/` Master Audio (`ma/` channel sets and embedded downmixes, `xll/` the lossless coder, `ma_core`, `native`); `express/` DTS Express; `container/` the extension substream (`exss`), the `.dtshd` file (`dtshd`) and the Bitstream-tab metadata (`mixmeta`). |
| `sw-codec-pcm` | Audio files: `wav`, `aiff`, `caf`, `flac`, `ogg`, `alac`, `tta`, `wavpack/`, `monkeys/`, `als/`, `dsd/`, `dcp/`. Each writer implements one small trait, `BlockWriter`: it is handed blocks of samples and writes its file. |
| `sw-backend-ffmpeg` | Running FFmpeg when a format has no native encoder or an input needs converting; downmix rendering. |
| `sw-backend-dee` | Driving a Dolby Encoding Engine you supply, for Atmos: `find`, `input` (what kind of master), `job` (the options), `profile` (the XML job file), `alias` (paths with spaces), `run`, `info`. |
| `sw-verify` | Reading files back: one module per stream type (`ac3`, `thd`, `dts/`, `audiofile/` with a module per file format), a deep mode with a full decode (`deep/`), and the DTS verifier as a second opinion if it is installed. |
| `sw-tools` | StreamTools: `dts_index` and `truehd_index` read a stream into frames; `cut`, `join`, `container` (the `.dtshd` wrapper), `timecode` (re-stripe), `silence`, `check`. |
| `sw-queue` | The job queue (`lib`) and the job runner `run/`: `spec` (the job as data), `sources` (opening the inputs), `encoders` (one function per encoder family, and the check of the result), `log`, and `execute` itself; `threads` applies the thread limit of a job; `measure` (loudness of the inputs) and `preview` (an excerpt encoded, decoded, folded to stereo and lined up with the source, for the app's Preview button). |
| `sw-cli` | The `subwoofa` command: `args` (every option), one module per command (`encode`, `verify`, `tools`, `atmos`, `downmix`, `listing`), and a `main` that dispatches. `encode` builds the same job spec as the app and hands it to `sw-queue`. |
| `app/` | The window. `app/ui` is plain HTML, CSS and JavaScript, one script per part of the page (`core`, `layout`, `job`, `bitstream`, `themes`, `tabs`, `presets`, `start`, `streamtools`, `verify`, `settings`, `atmos`; `main` starts it). `app/src-tauri` is the Rust side that exposes the crates to it: `catalog`, `sources`, `jobs`, `downmix`, `streamtools`, `toolpaths` and `atmos`. |

## Adding a format

1. **Name it** in `sw-core/src/formats.rs`: a `StreamType`, its id, display name, extension, rates, channel limit,
   and the destinations that offer it. The app lists whatever the matrix says.
2. **Write the encoder** in the matching codec crate. For an audio file, add a module to `sw-codec-pcm` with a writer
   that implements `BlockWriter`, a `Container` variant, and the line in `encode_to_file` that creates it.
3. **Connect it**: `sw-queue/src/run.rs` maps the stream type to the encoder; the app needs a plate in
   `app/ui/logos/<id>.svg` and, if it has settings, a case in `syncPcmFormat` in `app/ui/layout.js`.
4. **Check it**: a round-trip test that decodes the output with an independent decoder (usually FFmpeg), a case in
   `sw-verify`, a row in `docs/FORMATS.md`, a line in `CHANGELOG.md`.

## Conventions

- Encoders are written from public specifications and from watching what real streams contain; see
  [LEGAL.md](LEGAL.md). Constants and tables say where they come from.
- The code follows the standard at the end of [TODO.md](TODO.md), and is formatted by `cargo fmt` (CI checks it).
- Output must not change by accident: where an encoder was made faster or reorganised, the files before and after
  were compared byte for byte.
