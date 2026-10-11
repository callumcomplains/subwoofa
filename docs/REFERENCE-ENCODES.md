# Reference encodes wanted from the original DTS-HD Master Audio Suite encoder

Several features of the original encoder's **Bitstream** tab write metadata whose exact bits are not published. Subwoofa
can only copy them faithfully from the original's own output. Each encode below isolates one setting. Use the same short
input (5 to 10 seconds of anything with signal, the same file every time) and the same output folder. The `.dtshd` files
are all that is needed (the `_log.txt` files the encoder writes beside them are useful too).

| # | Destination / stream / layout | Setting to change | Name it |
|---|---|---|---|
| 1 | Blu-ray Secondary Audio, DTS Express, 1.0 mono, 48 kHz, 128 kbps | everything default (primary attenuation -6 dB, fades 0 s) | `x1_default` |
| 2 | same | Primary Audio Attenuation **-12 dB** | `x2_att12` |
| 3 | same | Primary Audio Attenuation **-30 dB**, Fade Down **2 s**, Fade Up **4 s** | `x3_att30_fades` |
| 4 | same | mono output mapping faders: C **-INF**, L **0**, R **-6**, Ls **-12**, Rs **-INF**, LFE **-INF** (attenuation back to -6, fades 0) | `x4_map` |
| 5 | same | **DTS Express Dialog Mode** on (everything else default) | `x5_dialog` |
| 6 | Blu-ray Primary Audio, DTS-HD Master Audio, 5.1, 48 kHz | Program Info = `Subwoofa test ABC 123` | `p1_info_short` |
| 7 | same | Program Info = about 1500 characters of text (any) | `p2_info_long` |
| 8 | same | Program Info empty (baseline for 6 and 7) | `p0_info_none` |
| 9 | Blu-ray Primary Audio, DTS-HD Master Audio, 7.1 L R C LFE Ls Rs Lw Rw | **Enable Wide 7.1 Remapping** on | `w1_wide_on` |
| 10 | same | Wide 7.1 Remapping off | `w0_wide_off` |
| 11 | Blu-ray Primary Audio, DTS-HD Master Audio, 5.1 | Seamless Branching: **Single Clip** | `s1_single_clip` |
| 12 | same | Seamless Branching: **Use CSV Branch Points** with a CSV of two timecodes inside the clip | `s2_csv` |
| 13 | same | Seamless Branching off | `s0_off` |

What is looked for: where the settings land in the extension-substream header (mixing metadata, additional text,
speaker remapping sets) and in the frames themselves; for 11 to 13 whether the streams differ at all (the manual says
the encoder only switches an internal filter off at the branch points, which may leave no trace in the stream).

Priority if time is short: 1, 2, 3 (attenuation and fades), then 6 and 8 (Program Info), then 9 and 10 (Wide 7.1), then 4.

## Everything else a reference unlocks

Besides the 13 Bitstream-tab encodes above, these files (from the original encoder or another licensed tool) would
let Subwoofa finish features it cannot check on its own. None is urgent; each is independent.

| Unlocks | What to send | How to make it |
|---|---|---|
| HRA 5.1 at 96 kHz in the commercial layout | one `.dtshd` and its `_log.txt` | original encoder: Blu-ray, DTS-HD High Resolution Audio, 5.1, 96 kHz, 5376 kbps, "Using 96/24 Core" off, any 10 s of music |
| HRA 6.1 | one `.dtshd` | same but 6.1 (discrete and matrix if both are offered) at 48 kHz, 3018 kbps |
| HRA 7.1 at 48 kHz in the commercial layout (checks my 48 kHz XBR split against the original's) | one `.dtshd` | 7.1 at 48 kHz, 3018 kbps |
| DVD-Audio disc structure | a DVD-Audio disc, or its `AUDIO_TS` folder (the `.AOB` files) | any commercial DVD-Audio, ideally one with MLP and one with LPCM |
| Atmos Theatrical | one Atmos cinema track file (`.mxf`, IAB) | from a DCP or from Dolby's cinema tools; a few seconds is enough |
| DTS:X encoding | DTS's licensed encoder, plus a DTS:X `.dts` or `.dtshd` made with it | the encoder from your lecturer, any short 7.1.4 programme |
| AAF import | AAF files from your editor | one with a few mono clips, one with a stereo clip, one with embedded and one with linked audio |
| E-AC-3 enhanced coupling | a Dolby reference decoder, or a stream with enhanced coupling and the matching decode | only if one comes your way |
| MA 192 kHz band 1 (my band 1 is 3.4% larger than the original's) | nothing more: the existing `NO WAY THIS WORKS.dtshd` is enough, it needs a segment-level parse | - |
| TrueHD stereo (0.7% larger than Dolby's) | nothing more; the Deep Purple files are enough | - |
