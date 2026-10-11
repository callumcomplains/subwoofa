# Roadmap

## Upcoming work, in order of importance

Estimates are focused working time for the work itself (writing, testing, documenting), not calendar time. "Needs" means the item cannot be finished or verified without it.

### Now (no outside input needed)
| # | Item | Estimate |
|---|---|---|
| 1 | Quality of life: speed pass on HRA 96 kHz and MA 192 kHz; a one-command regression sweep over every native DTS path (encode, verify, FFmpeg decode); clearer format notes and an HRA bit split readout in the GUI | 3 h |
| 2 | PCM outputs: WAV/BWF (extensible, 16/24/32-bit integer, 32/64-bit float), RF64 for files over 4 GB, AIFF, CAF, multichannel FLAC | 4 h |
| 3 | Probe the Dolby Encoding Engine for AC-4 job types (decides how AC-4 is done) | 0.5 h |
| 4 | Lossless size work: MA band 1 (reuse of previous segment parameters, common parameters); TrueHD stereo matrix with LSB bypass | 5 h |
| 5 | Reference checklist: exactly which encodes unlock each blocked item, with the settings to use | 0.5 h |

### Next (public specs, verifiable here)
| # | Item | Estimate |
|---|---|---|
| 6 | DCP audio: 24-bit PCM at 48/96 kHz, up to 16 channels, MXF track files (SMPTE and Interop); checked with FFmpeg and asdcplib, not with a cinema server | 8 h |
| 7 | SACD as files: PCM to DSD (sigma-delta modulator), `.dsf` / `.dff` output; checked with FFmpeg and by listening. Disc image authoring and DST compression are out (proprietary) | 7 h |
| 8 | AC-4 through DEE, if item 3 finds job types (as Atmos is done) | 4 h |

### Blocked on reference files or tools
| # | Item | Needs | Estimate once unblocked |
|---|---|---|---|
| 9 | Bitstream tab features | the reference encodes listed in `docs/REFERENCE-ENCODES.md` | 1-2 days |
| 10 | HRA 6.1; HRA 5.1 at 96 kHz in the commercial layout | a DTSHDMASE encode of each | 3 h each |
| 11 | DVD-Audio disc structure (AOB packs; the MLP inside is done) | one real DVD-Audio disc or its `.AOB` files | 1-2 days |
| 12 | Dolby Atmos Theatrical (IAB in MXF, SMPTE ST 2098-2), after item 6 | one real Atmos cinema track file | 2-3 days |
| 13 | DTS:X encoding | DTS's licensed encoder (detection in Verify is done) | 1-2 days |
| 14 | AAF import (deferred by request) | AAF files from your DAW | 1-2 days |
| 15 | E-AC-3 enhanced coupling | a reference decoder | open |

### Deferred (planned, not scheduled)
- **Make the code human-readable and easier to follow.** The codebase is dense: long lines, terse names, few comments on the why. A readability pass would cover, in this order: a short "how it fits together" guide per crate (what each file does, in plain language); module-level comments that explain each encoder's pipeline step by step; descriptive names for the one- and two-letter variables; splitting the very long functions in the DTS encoders and the verifier; and a line-length limit with automatic formatting. Output must stay byte-identical, so the existing tests and the regression sweep are the safety net. Estimate: 2-3 days, best done one crate at a time between feature work.

### Deferred (no realistic route yet)
- **Native AC-4 encoder:** a free ETSI standard but months of work, and no open decoder to verify against. Only worthwhile if item 3 finds nothing.
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
