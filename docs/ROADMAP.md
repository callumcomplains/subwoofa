# Roadmap

## Upcoming work, in order of importance

Estimates are focused working time for the work itself (writing, testing, documenting), not calendar time. "Needs" means the item cannot be finished or verified without it.

### Now (no outside input needed)
| # | Item | Estimate |
|---|---|---|
| 1 | ~~Quality of life: speed pass, regression sweep, format notes in the GUI~~ **done** | done |
| 2 | ~~PCM outputs: WAV/BWF, RF64, AIFF, CAF, multichannel FLAC~~ **done** | done |
| 3 | ~~Probe the Dolby Encoding Engine for AC-4 job types~~ **done: DEE 5.2.1 has no AC-4 encoder** (AC-4 is only a file type it passes through; no AC-4 filter among its audio filters) | done |
| 4 | ~~Lossless size work~~ **done as far as it goes**: channel pairing judged on whitened channels (-0.5% at 192 kHz); MA band 1 gap (3.4% to the original) and the TrueHD stereo gap (~0.7% to Dolby) were investigated and not closed, see the changelog | done |
| 5 | ~~Reference checklist~~ **done** (`docs/REFERENCE-ENCODES.md`) | done |

### Next (public specs, verifiable here)
| # | Item | Estimate |
|---|---|---|
| 6 | DCP audio: 24-bit PCM at 48/96 kHz, up to 16 channels, MXF track files (SMPTE and Interop); checked with FFmpeg and asdcplib, not with a cinema server | 8 h |
| 7 | ~~SACD as files: PCM to DSD, `.dsf` / `.dff` output~~ **done** (DSD64 and DSD128; checked with FFmpeg; still wants a listen). Disc image authoring and DST compression are out (proprietary) | done |
| 8 | Lossless audio file family: ~~ALAC, Ogg FLAC, TTA~~ **done**. Still to do: WavPack (FFmpeg's encoder is 3000 lines of C to port or rewrite from the format description), Monkey's Audio and MPEG-4 ALS (FFmpeg only has decoders for those, so the encoders would be written from their specifications and verified by FFmpeg's decoders) | WavPack 6 h, Monkey's 6 h, ALS 8 h |

### Blocked on reference files or tools
| # | Item | Needs | Estimate once unblocked |
|---|---|---|---|
| 9 | Bitstream tab features | the reference encodes listed in `docs/REFERENCE-ENCODES.md` | 1-2 days |
| 10 | ~~HRA 6.1 and HRA 5.1 at 96 kHz in the commercial layout~~ **done** (from reference encodes). HRA 6.1 at 48 kHz (XCH with XBR) is not written; it needs its own reference | done |
| 11 | DVD-Audio disc structure (AOB packs; the MLP inside is done) | one real DVD-Audio disc or its `.AOB` files | 1-2 days |
| 12 | Dolby Atmos Theatrical (IAB in MXF, SMPTE ST 2098-2), after item 6. Not through DEE: it can read an IAB MXF as input (useful for converting cinema Atmos to home Atmos) but has no IAB output | one real Atmos cinema track file | 2-3 days |
| 13 | DTS:X encoding | DTS's licensed encoder (detection in Verify is done) | 1-2 days |
| 14 | AAF import (deferred by request) | AAF files from your DAW | 1-2 days |
| 15 | E-AC-3 enhanced coupling | a reference decoder | open |

### Deferred (planned, not scheduled)
- **Make the code human-readable and easier to follow.** The codebase is dense: long lines, terse names, few comments on the why. A readability pass would cover, in this order: a short "how it fits together" guide per crate (what each file does, in plain language); module-level comments that explain each encoder's pipeline step by step; descriptive names for the one- and two-letter variables; splitting the very long functions in the DTS encoders and the verifier; and a line-length limit with automatic formatting. Output must stay byte-identical, so the existing tests and the regression sweep are the safety net. Estimate: 2-3 days, best done one crate at a time between feature work.

### Deferred (no realistic route yet)
- **AC-4:** the Dolby Encoding Engine cannot encode it (checked: AC-4 is a pass-through file type only). A native encoder would be a free ETSI standard but months of work with no open decoder to verify against.
- **MPEG-H 3D Audio:** the standard is paid; the encoder is commercial.
- **Auro-3D:** the Auro-Codec is proprietary. The discrete 9.1/10.1/11.1 layouts can be carried in MA, which is not an Auro stream.
- **Datasat (formerly DTS Theatrical):** status and documentation unknown; to be looked into when there is a concrete output file to target.
- **DVD-Audio, SACD and DCP disc/server testing:** needs real players and servers.

---

1. **Foundation** — workspace, layout/format matrix, CLI, FFmpeg backend, GUI, PCM reader, queue, downmix. *(done)*
   - Remaining: settings save/recall (`.swsettings`), folder-based batch encoding, BWF timecode fields in the GUI.
2. **Dolby non-Atmos native** — AC-3 *(done: block switching, 2/0 rematrixing; coupling done; next: exponent-strategy optimisation)*, then E-AC-3, MLP/TrueHD lossless 2.0–7.1.
3. **DTS** - core, ES (matrix and XCH discrete), 96/24 (X96), HRA (XBR) and the DTSHDMASE-style `.dtshd` container are *done*. Express/LBR is *done* for 1.0 to 5.1 (band-limit modes, 56 kbps per pair and up; tonal components still missing). HRA 7.1/7.0 (XXCH) is done, at 48 kHz and at 96 kHz (core + XXCH + X96). Remaining: HRA 6.1, ADPCM/VQ/transient modes in the core encoder, Huffman side information in X96.
4. **DTS-HD MA (XLL)** native *(done: core + lossless extension, 1.0 to 7.1 at 44.1 to 96 kHz, embedded downmix hierarchy; 96/24 core option done; 176.4/192 kHz up to 5.1 done, with a core (bit-exact, as the original encoder))*, plus StreamTools *(done: info, bit-rate chart, trim, split, append, replace and insert, re-stripe, add silence done)*; native Verify (simple and deep) for all formats done.
5. **Odd formats** — DVD-Audio MLP *(done)*, DTS-CD wav wrapper, multichannel FLAC/Opus/AAC, Ambisonics, 9.1/11.1 containers, remuxing.
6. **Immersive channel layouts** — NHK 22.2 (24-channel: 9 upper, 10 middle, 3 lower, 2 LFE) as an input layout *(done: 24-file import and panning downmix to any layout)*; still to do: carriage in lossless/PCM containers (e.g. multichannel FLAC/WAV, MLP/TrueHD where channel counts allow, DTS-HD XLL channel sets).
7. **Atmos** — ADM BWF import and viewer, TrueHD Atmos / DD+ JOC via Dolby Encoding Engine.

## Feature parity checklist with the legacy encoder UX
Audio tab (format cascade, input table, dialnorm, bit rate, ES options, timecode/reference TC), Bitstream tab (secondary-audio mix metadata, AAF, seamless branching CSV, program info, Express dialog mode, wide-7.1 remap), Downmix tab (7.x→5.1, 5.1→2.0, saturation check), queue (concurrency, verify, per-job log), prefs (auto verify, auto PBR, BWF TC, folder-based encode), presets.
