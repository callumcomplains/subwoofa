# Roadmap

1. **Foundation** — workspace, layout/format matrix, CLI, FFmpeg backend, GUI, PCM reader, queue, downmix. *(done)*
   - Remaining: settings save/recall (`.swsettings`), folder-based batch encoding, BWF timecode fields in the GUI.
2. **Dolby non-Atmos native** — AC-3 *(done: block switching, 2/0 rematrixing; coupling done; next: exponent-strategy optimisation)*, then E-AC-3, MLP/TrueHD lossless 2.0–7.1.
3. **DTS** - core, ES (matrix and XCH discrete), 96/24 (X96), HRA (XBR) and the DTSHDMASE-style `.dtshd` container are *done*. Express/LBR is *done* for 1.0 to 5.1 (band-limit modes, 56 kbps per pair and up; tonal components still missing). HRA 7.1/7.0 (XXCH) is done. Remaining: HRA at 96 kHz and 6.1, ADPCM/VQ/transient modes in the core encoder, Huffman side information in X96.
4. **DTS-HD MA (XLL)** native *(done: core + lossless extension, 1.0 to 7.1 at 44.1 to 96 kHz, embedded downmix hierarchy; 96/24 core option done; next: 192 kHz)*, plus StreamTools *(done: info, bit-rate chart, trim, split, append, verify; replace and insert done; not done: re-stripe, add silence)*.
5. **Odd formats** — DVD-Audio MLP *(done)*, DTS-CD wav wrapper, multichannel FLAC/Opus/AAC, Ambisonics, 9.1/11.1 containers, remuxing.
6. **Immersive channel layouts** — NHK 22.2 (24-channel: 9 upper, 10 middle, 3 lower, 2 LFE) as an input layout *(done: 24-file import and panning downmix to any layout)*; still to do: carriage in lossless/PCM containers (e.g. multichannel FLAC/WAV, MLP/TrueHD where channel counts allow, DTS-HD XLL channel sets).
7. **Atmos** — ADM BWF import and viewer, TrueHD Atmos / DD+ JOC via Dolby Encoding Engine.

## Feature parity checklist with the legacy encoder UX
Audio tab (format cascade, input table, dialnorm, bit rate, ES options, timecode/reference TC), Bitstream tab (secondary-audio mix metadata, AAF, seamless branching CSV, program info, Express dialog mode, wide-7.1 remap), Downmix tab (7.x→5.1, 5.1→2.0, saturation check), queue (concurrency, verify, per-job log), prefs (auto verify, auto PBR, BWF TC, folder-based encode), presets.
