# Roadmap

1. **Foundation** — workspace, layout/format matrix, CLI, FFmpeg backend, GUI, PCM reader, queue, downmix. *(done)*
   - Remaining: settings save/recall (`.swsettings`), folder-based batch encoding, BWF timecode fields in the GUI.
2. **Dolby non-Atmos native** — AC-3 *(done: long blocks only; next: block switching, coupling, rematrixing, exponent-strategy optimisation)*, then E-AC-3, MLP/TrueHD lossless 2.0–7.1.
3. **DTS** — core (Digital Surround, 96/24, ES), DTS-HD container + HRA, DTSHDMASE legacy backend, Express/LBR.
4. **DTS-HD MA (XLL)** native *(done: core + lossless extension to 5.1 at 44.1/48 kHz; next: 96 kHz via the 64-band core synthesis, 7.1 with an embedded 5.1 downmix, speed)*, plus StreamTools (join/append/trim/split/restripe/silence/info/verify/PBR).
5. **Odd formats** — DVD-Audio MLP, DTS-CD wav wrapper, multichannel FLAC/Opus/AAC, Ambisonics, 9.1/11.1 containers, remuxing.
6. **Immersive channel layouts** — NHK 22.2 (24-channel: 9 upper, 10 middle, 3 lower, 2 LFE) as an input layout, with test material incoming; downmix to 7.1.4/5.1/2.0 and carriage in lossless/PCM containers (e.g. multichannel FLAC/WAV, MLP/TrueHD where channel counts allow, DTS-HD XLL channel sets).
7. **Atmos** — ADM BWF import and viewer, TrueHD Atmos / DD+ JOC via Dolby Encoding Engine.

## Feature parity checklist with the legacy encoder UX
Audio tab (format cascade, input table, dialnorm, bit rate, ES options, timecode/reference TC), Bitstream tab (secondary-audio mix metadata, AAF, seamless branching CSV, program info, Express dialog mode, wide-7.1 remap), Downmix tab (7.x→5.1, 5.1→2.0, saturation check), queue (concurrency, verify, per-job log), prefs (auto verify, auto PBR, BWF TC, folder-based encode), presets.
