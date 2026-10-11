# Engines and tools

Subwoofa has its own encoders, decoders and verifiers. A few jobs use a program you install yourself. Subwoofa
never ships or downloads them. **Settings > Engines** shows what it found and lets you point it elsewhere.

| Tool | What it is for | Without it |
|---|---|---|
| **FFmpeg** | Reading input files that are not WAV/AIFF (FLAC, MP3, MKV and so on) and writing formats Subwoofa has no encoder for | Native encoders and WAV/AIFF input keep working. Formats that need FFmpeg are greyed out ("needs FFmpeg"). A banner offers to set it up |
| **DTS-HD Master Audio Suite** (optional) | `DTSHDVerify.exe`, used as a second opinion in deep verify | Nothing is disabled. Deep verify skips that check and says so |
| **Dolby Encoding Engine** (`dee.exe`) | All Dolby Atmos encoding | The Atmos tab is locked until it is set |

## FFmpeg

Download a Windows build from [ffmpeg.org](https://ffmpeg.org/download.html) (any recent "essentials" or "full"
build). Either add its `bin` folder to your `PATH`, or open **Settings > Engines > FFmpeg** and choose `ffmpeg.exe`
or its folder. Press **Use PATH** to go back to automatic.

## DTS-HD Master Audio Suite

Only needed for the extra verifier. If the suite is installed in its default folder, Subwoofa finds it. Otherwise
choose the folder that holds `DTSHDVerify.exe` under **Settings > Engines > DTS-HD Master Audio Suite**.

## Dolby Encoding Engine

See [ATMOS.md](ATMOS.md).

## Command line

The CLI accepts `--ffmpeg <path>` per command and otherwise searches `PATH`.
