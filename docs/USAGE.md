# Using Subwoofa

Subwoofa turns per-channel PCM files (one mono WAV per speaker, or a multichannel WAV) into DTS and Dolby
streams. Everything below works from the app and from the `subwoofa` command line; the app drives the same
encoders.

## The app, in short

1. **Destination** decides which stream types are offered (Blu-ray primary, Blu-ray secondary, DVD-Video, DTS
   Music Disc, digital delivery, or generic for the Dolby formats). **Stream type**, **sample rate** and
   **bit rate** follow from it; the range shown next to the bit rate is what the encoder can honour.
2. Pick the **channel layout** on the left. The speaker diagram and the input table follow.
3. Fill the input table: drop the L file anywhere (or double-click a row) and the other channels are found by
   name. IDs recognised: `L R C LFE Ls Rs Lss Rss Lsr Rsr Cs Lw Rw Lh Rh ...` plus `FL FR FC SL SR BL BR`,
   `Lb Rb Cb`, `LFE2`, `Tfc Tfl ...`; numbering such as `ch01` and words such as `Front-Left` are ignored when
   matching siblings. A single multichannel file works too.
4. Choose the output folder and name, press **Encode**. Jobs run in the queue (cancel, log, verify per job).
5. **Batch from folder…** queues one job per complete set of channel files in a folder.
6. **Input layout** lets the files be richer than the output: choose NHK 22.2 and the 24 files are mixed down
   to the chosen layout while they are read (native encoders only).
7. **Downmix & clip check** previews a downmix and reports clipping before you commit to a long encode.
8. **StreamTools** (mode switch at the top): info and bit-rate chart, trim, split, append, replace/insert,
   verify.

Settings you use often can be saved as presets (Settings).

## Command line

```sh
subwoofa layouts                       # all channel layouts
subwoofa formats                       # destinations and their stream types

# 5.1 DTS-HD Master Audio with the compatible core, from a set of mono WAVs
subwoofa encode --stream dtshd-ma --dest bluray-primary --layout 5.1 \
  --auto-import Film_L.wav --engine native -o Film.dtshd

# lossless only (no core), 7.1
subwoofa encode --stream dtshd-ma --dest bluray-primary --layout 7.1-side-rear \
  --auto-import Film_L.wav --bitrate 0 --engine native -o Film71.dtshd

# DTS-HD High Resolution at 3018 kbps, DTS 96/24, DTS-ES, DTS Express (Blu-ray secondary audio)
subwoofa encode --stream dtshd-hra  --dest bluray-primary --layout 5.1 --bitrate 3018 --auto-import ... --engine native -o out.dtshd
subwoofa encode --stream dts-96-24  --dest bluray-primary --layout 5.1 --sample-rate 96000 --auto-import ... --engine native -o out.dts
subwoofa encode --stream dts-es     --dest bluray-primary --layout 6.1-discrete --auto-import ... --engine native -o out.dtshd
subwoofa encode --stream dts-express --dest bluray-secondary --layout 2.0 --bitrate 96 --auto-import ... --engine native -o out.dtshd

# DTS CD (a WAV that carries the DTS stream), Dolby formats
subwoofa encode --stream dts   --dest dts-music-disc --layout 5.1 --sample-rate 44100 --bitrate 1411 --auto-import ... --engine native -o disc.wav
subwoofa encode --stream ac3   --dest generic --layout 5.1 --bitrate 448 --auto-import ... --engine native -o out.ac3
subwoofa encode --stream eac3  --dest generic --layout 7.1-ls-rs-lsr-rsr --bitrate 768 --auto-import ... --engine native -o out.eac3
subwoofa encode --stream truehd --dest generic --layout 7.1-ls-rs-lsr-rsr --auto-import ... --engine native -o out.thd
subwoofa encode --stream mlp   --dest generic --layout 5.1 --sample-rate 96000 --auto-import ... --engine native -o out.mlp

# NHK 22.2 stems mixed down to 7.1, a 60 s excerpt
subwoofa encode --stream dtshd-ma --dest bluray-primary --layout 7.1-side-rear --source-layout 22.2 \
  --auto-import "Programme ch01 C.wav" --start 600 --duration 60 --engine native -o excerpt.dtshd

# StreamTools
subwoofa tools info Film.dtshd
subwoofa tools pbr Film.dtshd           # peak bit rate over time
subwoofa tools trim Film.dtshd -o cut.dtshd --start 60 --end 120
subwoofa tools split Film.dtshd -o part --at 600 1200
subwoofa tools append a.dtshd b.dtshd -o joined.dtshd
subwoofa tools replace Film.dtshd fix.dtshd --at 3600 -o Film_fixed.dtshd
subwoofa verify Film.dtshd              # structure, checksums, headers and the container (seconds)
subwoofa verify Film.dtshd --deep       # also decodes everything, measures the audio, lists every header
subwoofa verify Film.ac3 --json         # the report as JSON (the exit code is 0 only if it passed)
```

`--start` and `--duration` (seconds) encode an excerpt, which is the quickest way to try settings.

## What the formats need

| Format | Notes |
|---|---|
| DTS-HD MA | CBR core plus lossless extension; 1.0 to 7.1 at 44.1/48/88.2/96 kHz, 1.0 to 5.1 at 176.4/192 kHz; the core can be switched off |
| DTS-HD HRA | 48 kHz, 2046 to 5376 kbps total; up to 5.1 with XBR, 7.1 and 7.0 with XXCH |
| DTS 96/24 | 96 kHz source, core plus X96 |
| DTS-ES | 6.1 matrix (optional surround phase shift, rear -3 dB) or discrete (XCH) |
| DTS Express | 1.0 to 5.1, 48 kHz; minimum rates 56 kbps per channel pair, 30 for a lone channel; bandwidth narrows below 128 kbps per channel |
| DTS Music Disc | 44.1 kHz, 754 or 1411 kbps, written as a DTS-in-PCM WAV |
| AC-3 / E-AC-3 | up to 5.1 (E-AC-3 also 7.1 / 7.0); block switching and rematrixing |
| TrueHD / MLP | lossless; TrueHD to 7.1 with a stereo downmix substream, MLP for DVD-Audio |

Every DTS-HD file Subwoofa writes can be checked with DTS's own `DTSHDVerify.exe` (if installed) and decoded
by FFmpeg; lossless streams decode to exactly the source samples.


## Verify

`subwoofa verify <file>` (or the **Verify** mode in the app) reads a file the way a decoder would parse it and says what
is wrong with it. It does not need another program for the simple check.

**Simple verification** (a few seconds even for a feature film)
- AC-3 / E-AC-3: sync words, frame sizes, header values, both checksums (CRC1 and CRC2) of every frame, steady parameters,
  complete last frame.
- DTS / DTS-HD: core frame headers (and the extension headers the core announces), extension substream headers with their
  checksums, the sync word of every component (XBR, XXCH, X96, LBR, XLL), the lossless (XLL) common and channel set header
  checksums, the access unit structure, and for a `.dtshd` file the container: required chunks, frame count, programme
  length and sample rate against the stream, the frame offset table, the timecode and the stored silent frame (which is
  what lets the DTS tools add silence).
- TrueHD / MLP: access unit lengths, the parity nibble, the major sync checksum, every substream's parity and checksum
  byte, the substream directory, input timing, restart points.

**Deep verification** adds
- a full decode with FFmpeg under strict error detection (anything the decoder complains about is listed), the decoded
  length against what the headers say, and the MD5 of the decoded 24-bit audio;
- measurements of the decoded audio: per-channel peak, RMS, DC offset, unused low bits, full-scale samples, channels that
  are silent or identical, dropouts inside the programme;
- bit-rate statistics (sizes, average, peak over one second);
- for DTS-HD with a core: the core-only decode against the lossless decode (alignment and SNR);
- DTS's own `DTSHDVerify` as a second opinion when it is installed;
- a dump of every header in the first frame (core, extension substream, XLL, container chunks, major sync...).

Limits worth knowing: the DTS core format has no checksum over its audio data, so a flipped bit inside the audio of a
plain DTS file is only found by decoding (deep). The structure check proves the file is well-formed, not that the audio
is what you meant to encode; for lossless streams the encoder's own auto-verify compares the decode with the source.


## Dolby Atmos

Subwoofa has no Atmos encoder of its own (the object coding is not public). Atmos goes through the **Dolby Encoding
Engine (DEE)**, which you supply and must be licensed to use; Subwoofa never ships or copies it. It is found in
`C:\Program Files\Dolby Encoding Engine`, in the folder named by the `SUBWOOFA_DEE` variable, or where you point the
Atmos mode ("Locate dee.exe...").

- **TrueHD Atmos** and **Dolby Digital Plus Atmos** from an Atmos master written by Dolby's tools: a DAMF set (`.atmos`
  with its `.metadata` and `.audio` files beside it) or an ADM BWF `.wav`. DEE checks that masters were authored with
  Dolby tools and refuses others; Subwoofa does not try to get around that.
- **Dolby Digital Plus Atmos from a channel bed**: a multichannel WAV (up to 7.1.4) needs no master.
- **Convert** a master between ADM BWF, DAMF and IAB.

```
subwoofa atmos info master.atmos
subwoofa atmos encode master.atmos --format truehd --clusters 12 --presentation-8ch -o film.thd
subwoofa atmos encode bed_714.wav  --format ddp --bitrate 768 -o film.ec3
subwoofa atmos convert master.atmos --format adm -o master.wav
```

The result is checked straight away with Verify, which recognises Atmos in both formats (Digital Plus: the Joint Object
Coding flag and its complexity index; TrueHD: the fourth substream with the spatial-presentation flag).


### DTS:X

Verify recognises DTS:X (and IMAX Enhanced) in DTS-HD Master Audio files: the object data trails the lossless audio of
each frame behind a marker word, and the file is called DTS:X when the marker is in most frames. Subwoofa cannot
*encode* DTS:X: the object coding is not public and needs DTS's own licensed tool, which Subwoofa could drive as an
optional backend once one is available. The marker check was confirmed on a real 4K Blu-ray DTS:X track (marker in 99.7% of the lossless frames) and agrees
with FFmpeg's own profile report.
