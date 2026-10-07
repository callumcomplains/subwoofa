# Subwoofa

*Every channel, every codec.*

An open-source multichannel audio encoder with a proper GUI (planned), turning PCM stems from your DAW or editor into DTS and Dolby streams: AC-3, E-AC-3, TrueHD/MLP, DTS core/ES/96-24, DTS-HD HRA/MA/Express, and assorted odd conversions. Inspired by the workflow of the abandoned DTS-HD Master Audio Suite Encoder.

## Status

Early foundation. What works today:

| Area | State |
|---|---|
| Channel layouts (29), channel-ID parsing, auto-import by file name | done |
| Destination / stream type / sample rate / bit-rate validation matrix | done (first pass) |
| CLI `subwoofa layouts / formats / encode` | done |
| FFmpeg backend: AC-3, E-AC-3, DTS core | working |
| Native AC-3 encoder (own MDCT, exponent coding, bit allocation, CRCs), 1.0 to 5.1 | working; verified against a reference decoder, about 70x real time |
| Native WAV / BWF / RF64 / AIFF reader, stem-set channel mapping | done |
| Encode queue: progress, cancel, concurrency, auto-verify (lossless MD5), per-job logs | done |
| FFmpeg backend: TrueHD / MLP | working; 5.1 verified bit-exact lossless on real PCM |
| `--start` / `--duration` partial encodes | done |
| Downmix matrices (Lo/Ro, 7.x to 5.1), safe trim, clip check (CLI `downmix-check` and GUI) | done |
| Desktop GUI (Tauri): format cascade, layout picker, bit-rate presets, unlocked mode, downmix tab | done |
| More native encoders (E-AC-3, TrueHD/MLP, DTS core), DTS-HD (HRA/MA/Express), StreamTools | planned — see `docs/ROADMAP.md` |
| Dolby Atmos | deferred |

## Build

Rust (stable). On Windows this repo pins the GNU toolchain (`rust-toolchain.toml`) so no Visual Studio is required.

```sh
cargo test
cargo build
./target/debug/subwoofa encode --stream ac3 --layout 5.1 --auto-import Mix_L.wav -o out.ac3
```

Needs `ffmpeg` on `PATH` (or `--ffmpeg <path>`) for the FFmpeg backend.

## Legal

Subwoofa is not affiliated with or endorsed by DTS, Inc. or Dolby Laboratories. "DTS", "Dolby", "TrueHD", "Atmos" and related names are trademarks of their owners. Several formats are patent-encumbered in some jurisdictions; see `docs/LEGAL.md`. This project uses public specifications only and does not contain or redistribute proprietary code or documentation.
