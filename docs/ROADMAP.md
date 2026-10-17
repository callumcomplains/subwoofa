# Roadmap

What is done, what comes next, and what is waiting on something from outside. The detailed, live list is
[`TODO.md`](TODO.md); the history is in [`../CHANGELOG.md`](../CHANGELOG.md). Updated 2026-10-17.

## Done

- **Encoders, all written natively from public specifications:** AC-3 and E-AC-3 (up to 7.1), TrueHD and MLP (up to
  7.1, with the stereo presentation), DTS core, ES, 96/24, HD High Resolution (5.1 and 7.x, 48 and 96 kHz), HD Master
  Audio (1.0 to 7.1 at 44.1 to 192 kHz, immersive layouts up to 22.2), DTS Express, DTS Music Disc.
- **Audio files:** WAV/BWF/RF64, AIFF, CAF, FLAC and Ogg FLAC, Apple Lossless, TTA, WavPack, Monkey's Audio, MPEG-4 ALS,
  DSD (DSF/DFF) and DCP sound track files (MXF, SMPTE and Interop).
- **Dolby Atmos and Dolby's own encoders** through the Dolby Encoding Engine you supply.
- **Checking:** Verify for every format above (frame structure, checksums, deep decode, QC report, comparison with the
  source), plus recognition of DTS:X, Dolby Atmos (TrueHD and Digital Plus), AC-4 and DTS-UHD.
- **Tools:** StreamTools (trim, split, join, replace, re-stripe, silence, dialnorm), loudness and normalise, preview with
  A/B, presets, folder batches, watch folders, JSON job files, an installer with optional FFmpeg download.
- **Inputs:** mono stems, stereo pairs, multichannel files, NHK 22.2 mixed down to any layout.
- **Code:** three readability rounds, a speed round (all cores, Fast/Balanced/Best), a full review
  ([`REVIEW-2026-10-16.md`](REVIEW-2026-10-16.md)) with its findings fixed.

## Next (needs nothing from outside)

| Item | Why |
|---|---|
| E-AC-3 enhanced coupling | better quality at low E-AC-3 bit rates; checked with Dolby's decoder through the engine |
| TrueHD predictor search (IIR, mixed FIR orders, Huffman offsets) | the last 0.7-1.4% against Dolby's own TrueHD |
| Video and existing streams as inputs; frame-rate conversion (23.976/24/25) | common delivery work |
| Header edits without re-encoding (DRC profile, DTS/TrueHD metadata) | as already done for AC-3 dialnorm |
| Downmix design (BS.775 presets, loudness-matched stereo) | better stereo presentations |
| Preview leftovers (multichannel monitor, `subwoofa preview`), a headless encode server | automation |
| macOS and Linux builds, package-manager manifests | reach, once releases are signed and hosted |

## Waiting on outside material

| Item | Needs |
|---|---|
| HRA 6.1 at 48 kHz | a reference encode |
| DVD-Audio disc structure (AOB packs; the MLP inside is done) | a real DVD-Audio disc or its `.AOB` files |
| Dolby Atmos Theatrical (IAB in MXF) | a real Atmos cinema track file |
| AAF import | AAF files from a DAW |
| Code signing of the installer | an account with a signing service |
| Real-hardware checks (players, a cinema server, a DVD-Audio player) | the hardware |

## Not possible here

- **DTS:X encoding:** needs DTS's licensed encoder (DTS:X is detected in Verify).
- **AC-4 and DTS-UHD encoding:** neither the installed Dolby Encoding Engine nor FFmpeg has an encoder or a decoder for
  them, so an encoder written here could not be checked. Verify recognises both and checks their framing. AC-4 would
  become possible through an engine build with Dolby's IMS AC-4 module.
- **MPEG-H 3D Audio** (paid standard, commercial encoder), **Auro-3D** (proprietary codec), **Datasat** (no public
  documentation).
