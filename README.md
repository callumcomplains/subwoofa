# Subwoofa

*Every channel, every codec.*

An open-source multichannel audio encoder with a proper GUI, turning PCM stems from your DAW or editor into DTS and Dolby streams: AC-3, E-AC-3, TrueHD/MLP, DTS core/ES/96-24, DTS-HD HRA/MA/Express, and assorted odd conversions. Inspired by the workflow of the abandoned DTS-HD Master Audio Suite Encoder.

## Status

Early foundation. What works today:

| Area | State |
|---|---|
| Channel layouts (30, including NHK 22.2 as an input), channel-ID parsing, auto-import by file name | done |
| Destination / stream type / sample rate / bit-rate validation matrix | done (first pass) |
| CLI `subwoofa layouts / formats / encode` | done |
| FFmpeg backend: AC-3, E-AC-3, DTS core | working |
| Native AC-3 and E-AC-3 (Dolby Digital Plus) encoders, 1.0 to 5.1 (E-AC-3 also 7.1 / 7.0 through a dependent substream), any E-AC-3 bit rate 8 to 1024 kbps | working; verified against a reference decoder, about 50x real time |
| Native TrueHD lossless encoder (up to 7.1, 44.1 to 192 kHz) with a real stereo-downmix substream (matrices + LSB bypass), bit-exact against a reference decoder | working; smaller than FFmpeg's on test material |
| Native TrueHD analyser (`subwoofa stream-info file.thd`): where the bits go, which tools a stream uses | done |
| Native DTS core (Digital Surround) encoder, 1.0 to 5.1 at 32 to 48 kHz (no ADPCM/VQ yet), verified by decoding | working |
| Native DTS-ES: 6.1 / 6.0 Matrix (with optional surround phase shift and rear -3 dB) and Discrete (XCH extension) | working |
| Native DTS 96/24 (core + X96 extension, 96 kHz input) | working |
| Native DTS-HD High Resolution (core + XBR extension, 2046-5376 kbps, 48 kHz) | working |
| Native DTS-HD Master Audio (CBR core + lossless extension, 1.0 to 7.1 at 44.1 to 96 kHz, embedded 5.1 downmix for 6.x/7.x, optional core): bit-exact through FFmpeg, passes DTS's own verifier, `.dtshd` container; a full feature film encodes about 5% smaller than the legacy encoder | working |
| Native MLP (DVD-Audio, mono to 5.1) | working |
| StreamTools: info, bit-rate chart, trim, split, append, verify for DTS / DTS-HD / TrueHD / MLP (GUI mode and `subwoofa tools`) | working |
| Native WAV / BWF / RF64 / AIFF reader, stem-set channel mapping | done |
| Encode queue: progress, cancel, concurrency, auto-verify (lossless MD5), per-job logs | done |
| FFmpeg backend: TrueHD / MLP | working; 5.1 verified bit-exact lossless on real PCM |
| `--start` / `--duration` partial encodes | done |
| Downmix matrices (Lo/Ro, 7.x to 5.1), safe trim, clip check (CLI `downmix-check` and GUI) | done |
| Desktop GUI (Tauri): format cascade, layout picker, bit-rate presets, unlocked mode, downmix tab | done |
| Native DTS-HD Express (LBR): 1.0 to 5.1 with LFE at 48 kHz, 48-510 kbps, `.dtshd` (modest quality; no tonal components yet) | working |
| AC-3 block switching (spec transient detector) and 2/0 rematrixing | working |
| NHK 22.2 input (24 mono files, auto-import) mixed down to any supported layout by panning | working |
| StreamTools replace / insert (CLI and GUI) | working |
| DTS Music Disc (DTS-in-PCM WAV at the CD rate), folder batch encode | working |
| MA with a 96/24 core, HRA at 96 kHz and above 7.1, MA at 192 kHz, AC-3 coupling, Express band limiting | planned - see `docs/ROADMAP.md` |
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
