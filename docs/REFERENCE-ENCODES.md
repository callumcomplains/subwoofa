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
