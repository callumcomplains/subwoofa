# Roadmap

1. **Foundation** — workspace, layout/format matrix, CLI, FFmpeg backend, GUI, PCM reader, queue, downmix. *(done)*
   - Remaining: settings save/recall (`.swsettings`), folder-based batch encoding, BWF timecode fields in the GUI.
2. **Dolby non-Atmos native** — AC-3 *(done: long blocks only; next: block switching, coupling, rematrixing, exponent-strategy optimisation)*, then E-AC-3, MLP/TrueHD lossless 2.0–7.1.
3. **DTS** — core (Digital Surround, 96/24, ES), DTS-HD container + HRA, DTSHDMASE legacy backend, Express/LBR.
4. **DTS-HD MA (XLL)** native, plus StreamTools (join/append/trim/split/restripe/silence/info/verify/PBR).
5. **Odd formats** — DVD-Audio MLP, DTS-CD wav wrapper, multichannel FLAC/Opus/AAC, Ambisonics, 9.1/11.1 containers, remuxing.
6. **Atmos** — ADM BWF import and viewer, TrueHD Atmos / DD+ JOC via Dolby Encoding Engine.

## Feature parity checklist with the legacy encoder UX
Audio tab (format cascade, input table, dialnorm, bit rate, ES options, timecode/reference TC), Bitstream tab (secondary-audio mix metadata, AAF, seamless branching CSV, program info, Express dialog mode, wide-7.1 remap), Downmix tab (7.x→5.1, 5.1→2.0, saturation check), queue (concurrency, verify, per-job log), prefs (auto verify, auto PBR, BWF TC, folder-based encode), presets.
