# Formats and features

What Subwoofa can write and read, and how far each piece has been tested. "Verified" means checked by decoding the result with an independent decoder (FFmpeg) or, for DTS, with DTS's own verifier where it is installed.

Nothing here needs a third-party program except where it says FFmpeg or the Dolby Encoding Engine; see [ENGINES.md](ENGINES.md).

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
| Audio file outputs: WAV/BWF (with RF64 and time reference), AIFF/AIFF-C, CAF, a native multichannel FLAC encoder (also in Ogg), Apple Lossless (M4A), TTA and DSD files (DSF and DSDIFF, DSD64/DSD128, up to 5.1) (smaller than FFmpeg's best setting on test material), 16/24/32-bit integer and 32/64-bit float | working; bit-exact through FFmpeg |
| Native DTS-HD High Resolution, 2046-5760 kbps: core + XBR for up to 5.1 at 48 kHz, core + XXCH for 7.1 and 7.0 at 48 kHz, core + X96 for 5.x and 6.x (matrix or discrete) and core + XXCH + X96 for 7.x at 96 kHz (the layout of the commercial encoder's streams) | working |
| Immersive layouts 5.1.2, 5.1.4, 7.1.2, 7.1.4, 9.1.6 and NHK 22.2 (24 channels) as lossless DTS-HD MA (no core; hierarchical channel sets of up to 8 channels), decoded bit-exact by FFmpeg | working |
| Native DTS-HD Master Audio (CBR core + lossless extension, 1.0 to 7.1 at 44.1 to 96 kHz, embedded 5.1 downmix for 6.x/7.x, optional core or 96/24 core): bit-exact through FFmpeg, passes DTS's own verifier, `.dtshd` container; a full feature film encodes about 5% smaller than the legacy encoder | working |
| Native MLP (DVD-Audio, mono to 5.1) | working |
| StreamTools: info, bit-rate chart, trim, split, append, replace, re-stripe, add silence for DTS / DTS-HD / TrueHD / MLP (GUI mode and `subwoofa tools`) | working |
| **Dolby Atmos** through the Dolby Encoding Engine you supply and are licensed to use (nothing of it is shipped): TrueHD Atmos and Dolby Digital Plus Atmos from Dolby-authored masters (DAMF `.atmos`, ADM BWF), Dolby Digital Plus Atmos from a channel bed up to 7.1.4, master conversion; progress, cancel and an automatic check of the result (GUI "Atmos" mode and `subwoofa atmos`) | working for Digital Plus (tested); TrueHD Atmos built but not yet run on a real master |
| **Verify** (own implementation, no other program needed): AC-3, E-AC-3, DTS, DTS-HD (.dtshd container, core, XBR, XXCH, X96, XLL, LBR), TrueHD, MLP. Simple mode checks sync, sizes, checksums, headers and the container; deep mode adds a full decode, audio measurements and a dump of every header (GUI "Verify" mode and `subwoofa verify [--deep]`) | working |
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
| Dolby Atmos Theatrical (cinema IAB), DTS:X encoding | not planned for this release |
