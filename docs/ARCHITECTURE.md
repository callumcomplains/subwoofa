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

A **job spec** (`sw_queue::JobSpec`) is plain data: the destination, stream type, layout, input files, bit rate,
rate, and every option. The app builds one from its controls, the command line from its flags, and a preset file is
one saved to disk. `sw-queue::run::execute` turns it into an encode and a log.

## The crates

| Crate | What it does |
|---|---|
| `sw-core` | The vocabulary: channel layouts and IDs (`layout`, `channel`), the capability matrix of destinations, stream types, rates and limits (`formats`), the Bitstream-tab settings (`bitstream`), downmix matrices (`downmix`), finding sibling channel files (`autoimport`), the version (`version`). |
| `sw-pcm` | Reading input audio (WAV / RF64 / AIFF) into `f32` stems and mapping files to speaker slots. |
| `sw-codec-ac3` | Dolby Digital and Digital Plus: transform (`mdct`, `blocksw`), bit allocation and the frame writer (`lib`), tables. |
| `sw-codec-truehd` | TrueHD and MLP lossless encoder, plus an analyser that walks a stream the way a decoder would. |
| `sw-codec-dts` | The DTS family. Core: `encoder`, `qmf`, `native`. Extensions: `x96` / `dts96` (96/24), `xbr` / `hra*` (High Resolution), `xll*` / `ma*` (Master Audio), `lbr*` (Express). Containers: `exss`, `dtshd`. Metadata: `mixmeta`. |
| `sw-codec-pcm` | Audio files: `wav`, `aiff`, `caf`, `flac`, `ogg`, `alac`, `tta`, `wavpack/`, `monkeys/`, `als/`, `dsd/`, `dcp/`. Each writer implements one small trait, `BlockWriter`: it is handed blocks of samples and writes its file. |
| `sw-backend-ffmpeg` | Running FFmpeg when a format has no native encoder or an input needs converting; downmix rendering. |
| `sw-backend-dee` | Driving a Dolby Encoding Engine you supply, for Atmos. |
| `sw-verify` | Reading files back: structure checks for each stream type, a deep mode with a full decode, and the DTS verifier as a second opinion if it is installed. |
| `sw-tools` | StreamTools: info, bit-rate chart, trim, split, append, replace, re-stripe, silence. |
| `sw-queue` | The job queue and `run::execute`. |
| `sw-cli` | The `subwoofa` command. |
| `app/` | The window: `app/ui` is plain HTML, CSS and JavaScript; `app/src-tauri` is the Rust side that exposes the crates to it. |

## Adding a format

1. **Name it** in `sw-core/src/formats.rs`: a `StreamType`, its id, display name, extension, rates, channel limit,
   and the destinations that offer it. The app lists whatever the matrix says.
2. **Write the encoder** in the matching codec crate. For an audio file, add a module to `sw-codec-pcm` with a writer
   that implements `BlockWriter`, a `Container` variant, and the line in `encode_to_file` that creates it.
3. **Connect it**: `sw-queue/src/run.rs` maps the stream type to the encoder; the app needs a plate in
   `app/ui/logos/<id>.svg` and, if it has settings, a case in `syncPcmFormat` in `app/ui/app.js`.
4. **Check it**: a round-trip test that decodes the output with an independent decoder (usually FFmpeg), a case in
   `sw-verify`, a row in `docs/FORMATS.md`, a line in `CHANGELOG.md`.

## Conventions

- Encoders are written from public specifications and from watching what real streams contain; see
  [LEGAL.md](LEGAL.md). Constants and tables say where they come from.
- The code follows the standard at the end of [TODO.md](TODO.md), and is formatted by `cargo fmt` (CI checks it).
- Output must not change by accident: where an encoder was made faster or reorganised, the files before and after
  were compared byte for byte.
