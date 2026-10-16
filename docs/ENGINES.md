# Engines and tools

Subwoofa has its own encoders, decoders and verifiers. A few jobs use a program you install yourself. Subwoofa
never ships or downloads them. **Settings > Engines** shows what it found and lets you point it elsewhere.

| Tool | What it is for | Without it |
|---|---|---|
| **FFmpeg** | Reading input files that are not WAV/AIFF (FLAC, MP3, MKV and so on) and writing formats Subwoofa has no encoder for | Native encoders and WAV/AIFF input keep working. Formats that need FFmpeg are greyed out ("needs FFmpeg"). A banner offers to set it up |
| **DTS-HD Master Audio Suite** (optional) | `DTSHDVerify.exe`, used as a second opinion in deep verify | Nothing is disabled. Deep verify skips that check and says so |
| **Dolby Encoding Engine** (`dee.exe`) | All Dolby Atmos encoding | The Atmos tab is locked until it is set |

## FFmpeg

The installer offers to fetch FFmpeg for you (a checkbox on its Extras page), and **Settings > Engines > FFmpeg >
Download FFmpeg** does the same later: it downloads the current "essentials" build from gyan.dev, checks it against
its published SHA-256 and keeps it in `%LOCALAPPDATA%\Subwoofa\ffmpeg`. Subwoofa looks there after `PATH`.

Or download a Windows build from [ffmpeg.org](https://ffmpeg.org/download.html) (any recent "essentials" or "full"
build). Either add its `bin` folder to your `PATH`, or open **Settings > Engines > FFmpeg** and choose `ffmpeg.exe`
or its folder. Press **Use PATH** to go back to automatic.

## DTS-HD Master Audio Suite

Only needed for the extra verifier. If the suite is installed in its default folder, Subwoofa finds it. Otherwise
choose the folder that holds `DTSHDVerify.exe` under **Settings > Engines > DTS-HD Master Audio Suite**.

## Dolby Encoding Engine

See [ATMOS.md](ATMOS.md).

### Beyond Atmos

With the engine installed, Subwoofa also uses it for these (all optional, all hidden without it):

- **Dolby's meter in Measure.** The Dolby meter finds the dialogue first and measures that, which is what a dialnorm describes; Subwoofa's own meter measures the whole programme. On the excerpt I tried, the two differed by a whole dialnorm step. Command line: `subwoofa loudness --dolby`, `subwoofa encode --auto-dialnorm --dolby-meter`.
- **Dolby's own encoder for Dolby Digital, Digital Plus and TrueHD** (Settings > General > "Use Dolby's encoder…", or `subwoofa encode --engine dolby`): mono, stereo, 5.1 and 7.1 (Digital Plus and TrueHD only). Its TrueHD is ordinary lossless TrueHD; Subwoofa switches off the engine's 3 dB surround cut, which would otherwise change a finished mix. The engine measures the dialogue and writes the dialnorm from it. Useful as a reference to compare Subwoofa's encoder against (`subwoofa verify FILE --against SOURCE`).
- **StreamTools > Dolby engine** (and `subwoofa dolby ...`): decode Dolby Digital (Plus) to WAV, transcode to Digital Plus, trim and join E-AC-3 without re-encoding, and correct the loudness of a WAV to a broadcast preset (ATSC A/85, EBU R 128, Free TV OP-59, ARIB B32).

Limits of the engine as installed: channel reordering (`pcm_channel_order`) refuses content under 20 minutes, and this build has no AAC or AC-4 encoder, so Subwoofa does not offer them. Paths with spaces are handled by Subwoofa (the engine cannot read them); its work happens in a folder without spaces, in the system temporary folder or a shared one.

## Command line

The CLI accepts `--ffmpeg <path>` per command and otherwise searches `PATH`.
