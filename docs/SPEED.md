# Speed

How fast Subwoofa encodes, next to FFmpeg and the original DTS-HD Master Audio Suite encoder (DTSHDMASE). Measured
on 2026-10-12. These are single-machine numbers: use them to see the order of magnitude, not as a benchmark suite.

## What was measured

- **Material:** 120 seconds of 5.1 film audio (24 bit, 48 kHz), six mono stems; 2.0 for Monkey's Audio, which only
  does stereo.
- **Machine:** AMD Ryzen 7 3700X (8 cores, 16 threads), 32 GB, Windows 11, SSD. Nothing else running.
- **Subwoofa:** the release build of the command-line tool (`subwoofa encode ... --engine native`), input as six mono
  WAV files. Median of three runs; the runs agree within 2%.
- **FFmpeg:** version 8.1, input as one 6-channel WAV, default encoder settings except the bit rate; median of
  three runs. (FFmpeg's own DTS and TrueHD encoders are marked experimental and need `-strict -2`.)
- **DTSHDMASE:** the installed encoder engine driven headlessly, one run per case. Its time is wall time from the
  moment the job is sent until its log says it finished, so it includes about 1.8 seconds of polling delay. Settings
  are the commercial encoder's own defaults for that format on Blu-ray (primary audio; secondary for Express).
- "**x real time**" is 120 s divided by the time taken: 10x means two minutes of audio in twelve seconds.

## Results

Time in seconds (x real time) and output size in MB.

| Format | Subwoofa | FFmpeg | DTSHDMASE |
|---|---|---|---|
| Dolby Digital (AC-3) 640 kbps | 2.2 s (55x), 9.6 MB | 0.7 s (176x), 9.6 MB | |
| Dolby Digital Plus (E-AC-3) 640 kbps | 1.9 s (63x), 9.6 MB | 0.6 s (185x), 9.6 MB | |
| Dolby TrueHD (lossless) | 12.4 s (10x), **54.3 MB** | 2.6 s (46x), 55.7 MB | |
| DTS core 1509 kbps | 12.6 s (10x), 22.6 MB | 5.6 s (22x), 22.6 MB | 7.5 s (16x), 22.6 MB |
| DTS-HD High Resolution 3018 kbps | 20.4 s (6x), 45.3 MB | not available | 8.5 s (14x), 45.3 MB |
| DTS-HD Master Audio (core + lossless) | 18.8 s (6x), **57.0 MB** | not available | 14.5 s (8x), 60.1 MB |
| DTS-HD Express 256 kbps | **19.4 s (6x)**, 3.8 MB | not available | 32.0 s (4x), 3.8 MB |
| FLAC | 7.8 s (15x), 51.4 MB | 0.6 s (196x), 51.7 MB | |
| WavPack | 2.9 s (41x), **51.6 MB** | 1.4 s (85x), 55.1 MB | |
| Apple Lossless | 23.9 s (5x), 52.2 MB | 1.4 s (84x), 52.6 MB | |
| TTA | 2.3 s (52x), 55.3 MB | 0.8 s (143x), 55.2 MB | |
| MPEG-4 ALS | 25.8 s (5x), 50.8 MB | not available | |
| Monkey's Audio (stereo) | 1.7 s (69x), 16.7 MB | not available | |
| DCP sound (MXF) | 1.9 s (63x), 103.8 MB | not available | |
| WAV, 24-bit PCM (a plain copy) | 1.9 s (63x), 103.7 MB | 0.2 s (530x), 103.7 MB | |

Bold marks where Subwoofa is the smaller file or the faster program.

## Reading it honestly

- **FFmpeg is faster than Subwoofa on every format they share**, usually by 2 to 15 times. It is a mature, heavily
  optimised C codebase, and the formats it makes are mostly cheaper to encode than the ones we do at full effort.
- **Part of Subwoofa's time is a fixed cost.** Even a plain WAV copy takes 1.9 seconds, against 0.2 for FFmpeg,
  because Subwoofa reads six separate mono files and converts them to floating point first. That is about 1.7
  seconds of the figures above; on the fastest formats it is most of the time.
- **Where Subwoofa wins, it is on file size, not speed.** TrueHD is 2.4% smaller than FFmpeg's, WavPack 6% smaller,
  DTS-HD Master Audio 5% smaller than the original encoder's. FLAC, Apple Lossless and TTA come out within 1% of
  FFmpeg's.
- **Against the original DTS encoder:** Subwoofa is faster on Express (6x against 4x real time) and slower on the
  core (10x against 16x), High Resolution (6x against 14x) and Master Audio (6x against 8x). The two make the
  same-size files for core, High Resolution and Express, which is the point of matching the format.
- **Nothing here is slow for real use.** A two-hour film at 6x real time is 20 minutes. The slowest case, MPEG-4 ALS
  at 5x, is a two-hour film in about 25 minutes. Encodes of several streams can also run at once (Settings, "Encodes at
  the same time").
- **Express** got about three times faster in this round (the quantiser search now uses all cores). The other
  encoders are single-threaded, so more speed is available by the same route if it matters.

## Repeating it

The numbers come from a small script in the private research repository: it cuts the stems, runs each encoder with
`time.perf_counter`, and drives the original encoder through its job engine. To repeat it without that, cut a
2-minute 5.1 excerpt into six mono WAVs and time `subwoofa encode` against `ffmpeg -i six_channel.wav -c:a <codec>`.
