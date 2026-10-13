# Speed

How fast Subwoofa encodes, next to FFmpeg and the original DTS-HD Master Audio Suite encoder (DTSHDMASE). Measured
on 2026-10-12 to 14. These are single-machine numbers: use them to see the order of magnitude, not as a benchmark
suite.

## What was measured

- **Material:** 120 seconds of 5.1 film audio (24 bit, 48 kHz), six mono stems; 2.0 for Monkey's Audio, which only
  does stereo.
- **Machine:** AMD Ryzen 7 3700X (8 cores, 16 threads), 32 GB, Windows 11, SSD. Nothing else running.
- **Subwoofa:** the release build of the command-line tool (`subwoofa encode ... --engine native`), input as six mono
  WAV files, at the default speed (Balanced). "All cores" is the default; "one core" is `--threads 1`. Median of
  two runs (one for the single-core column); the runs agree within 2%.
- **FFmpeg:** version 8.1, input as one 6-channel WAV, default encoder settings except the bit rate; median of
  three runs. FFmpeg's encoders each run on one core. (Its DTS and TrueHD encoders are marked experimental and need
  `-strict -2`.)
- **DTSHDMASE:** the installed encoder engine driven headlessly, one run per case. Its time is wall time from the
  moment the job is sent until its log says it finished, so it includes about 1.8 seconds of polling delay. Settings
  are the commercial encoder's own defaults for that format on Blu-ray (primary audio; secondary for Express).
- "**x real time**" is 120 s divided by the time taken: 10x means two minutes of audio in twelve seconds.

## Results

Time in seconds (x real time) and output size in MB.

| Format | Subwoofa, all cores | Subwoofa, one core | FFmpeg | DTSHDMASE |
|---|---|---|---|---|
| Dolby Digital (AC-3) 640 kbps | **0.65 s (185x)**, 9.6 MB | 2.1 s (56x) | 0.7 s (176x), 9.6 MB | |
| Dolby Digital Plus (E-AC-3) 640 kbps | 0.61 s (197x), 9.6 MB | 1.9 s (63x) | 0.6 s (185x), 9.6 MB | |
| Dolby TrueHD (lossless) | **1.7 s (71x)**, **54.4 MB** | 10.9 s (11x) | 2.6 s (46x), 55.7 MB | |
| DTS core 1509 kbps | **2.4 s (50x)**, 22.6 MB | 11.9 s (10x) | 5.6 s (22x), 22.6 MB | 7.5 s (16x), 22.6 MB |
| DTS-HD High Resolution 3018 kbps | **4.7 s (26x)**, 45.3 MB | 27.3 s (4x) | not available | 8.5 s (14x), 45.3 MB |
| DTS-HD Master Audio (core + lossless) | **5.6 s (21x)**, **57.0 MB** | 24.2 s (5x) | not available | 14.5 s (8x), 60.1 MB |
| DTS-HD Express 256 kbps | **16.8 s (7x)**, 3.8 MB | 40.6 s (3x) | not available | 32.0 s (4x), 3.8 MB |
| FLAC | 0.94 s (128x), 51.4 MB | 3.7 s (32x) | 0.6 s (196x), 51.7 MB | |
| WavPack | **0.84 s (143x)**, **51.6 MB** | 1.9 s (62x) | 1.4 s (85x), 55.1 MB | |
| Apple Lossless | 1.7 s (71x), 52.3 MB | 11.6 s (10x) | 1.4 s (84x), 52.6 MB | |
| TTA | **0.50 s (240x)**, 55.3 MB | 1.4 s (85x) | 0.8 s (143x), 55.2 MB | |
| MPEG-4 ALS | 3.4 s (35x), 50.8 MB | 28.6 s (4x) | not available | |
| Monkey's Audio (stereo) | 0.42 s (286x), 16.7 MB | 1.4 s (87x) | not available | |
| DCP sound (MXF) | 0.74 s (162x), 103.8 MB | 0.9 s (132x) | not available | |
| WAV, 24-bit PCM (a plain copy) | 0.22 s (545x), 103.7 MB | 0.4 s (298x) | 0.2 s (530x), 103.7 MB | |

Bold marks where Subwoofa is the smaller file or the faster program.

## Reading it honestly

- **With every core, Subwoofa matches or beats FFmpeg on most formats and beats the original DTS encoder on every
  format it makes.** DTS core is 2.3 times quicker than FFmpeg's encoder and 3 times quicker than the original;
  Master Audio is 2.6 times quicker than the original.
- **On one core it does not.** FFmpeg's encoders are single-threaded and cheap, and Subwoofa's do more work to find
  smaller files: on one core TrueHD takes 4 times as long as FFmpeg, AC-3 3 times. The lead above comes from using
  the whole machine, and a machine that is busy with other encodes gives less of it back (Settings, Performance).
- **Where Subwoofa wins on size:** TrueHD is 2.4% smaller than FFmpeg's, WavPack 6% smaller, DTS-HD Master Audio 5%
  smaller than the original encoder's. FLAC, Apple Lossless and TTA come out within 1% of FFmpeg's.
- **Plain copies are at parity.** A WAV copy is 0.22 s against FFmpeg's 0.23 s. The floor of every number above is
  reading and converting the six input files, which now overlaps with encoding.
- **Express is the slowest** (7x real time on all cores, 3x on one). It is 2 times quicker than the original, but its
  frame search does not spread over cores as well as the others.
- **Nothing here is slow for real use.** A two-hour film at 7x real time is about 17 minutes; at 50x it is 2.5.

## The speed setting

Lossless encoders can always try one more predictor or block size, and the last few tries pay little. The speed
setting (`--speed fast|balanced|best`, next to the Encode button in the app) says how many to try. It changes how large
a lossless file comes out, never whether it is correct, and it does nothing for lossy formats, whose size is fixed
by the bit rate.

Measured on 20-second excerpts of film (5.1 and 7.1, 48 kHz) and music (5.1, 96 kHz). Size is against Best; speed is
the one-core time against Best.

| Format | Balanced: size | Balanced: speed | Fast: size | Fast: speed |
|---|---|---|---|---|
| Dolby TrueHD / MLP | 0.0% to 0.6% larger | 4 to 4.5 times quicker | 0.7% to 4.5% larger | 6.5 to 8 times quicker |
| DTS-HD Master Audio | 0.02% to 0.21% larger | 1.8 to 2.2 times quicker | 0.10% to 0.26% larger | 2.1 to 2.7 times quicker |
| FLAC | 0.00% larger | 1.2 to 1.4 times quicker | 0.24% to 5.4% larger | 1.7 to 1.8 times quicker |
| Apple Lossless | 0.03% to 0.07% larger | 1.7 to 1.9 times quicker | 0.2% to 0.6% larger | 4 to 4.8 times quicker |
| MPEG-4 ALS | 0.02% to 0.06% larger | 1.9 to 2 times quicker | 0.1% to 5.3% larger | 6.5 to 7.6 times quicker |

What each level skips:

- **TrueHD:** Balanced drops the two filter families that cost most of the time (a fixed two-pole stage in front of
  the predictor, and a moving-average stage after it). Fast also drops cross-channel prediction.
- **Master Audio:** Balanced drops the covariance-method predictor. Fast also tries fewer predictor orders.
- **FLAC:** Balanced tries fewer predictors and partition depths and picks the stereo mode from the signals; Fast
  tries one predictor.
- **Apple Lossless:** fewer predictor orders; Balanced and Fast choose the stereo mode from the signals.
- **ALS:** Balanced codes one predictor order per block instead of four; Fast also stops cutting frames into
  shorter blocks.

The formats not listed (WavPack, TTA, Monkey's Audio, DCP, PCM, DSD and all the lossy ones) have one setting.

Best is exactly what earlier versions made. Where a speed-up kept the output the same, the files before and after
were compared byte for byte (everything that is not a setting: the frame-parallel encoders, the faster filter banks
and searches).

## Cores

`--threads N` (Settings, Performance: "Processor cores per encode") limits an encode to N cores; 0, the default, uses
all of them. The output does not depend on it: the same bytes come out with 1 core or 16.

Most encoders cut the work into independent pieces: AC-3 and DTS frames, TrueHD restart intervals, Master Audio core
frames and lossless frames, and the blocks of every audio-file format. What cannot be cut stays in order: a filter
bank's state, the decoder emulation Master Audio needs, and the file writing. Reading the next block of input
overlaps with encoding the current one.

## Repeating it

The numbers come from small scripts in the private research repository: `speed.py` cuts nothing and times each
encoder (`speed.py ours <speed> <threads>`), `explore.py` and `effort_eval.py` run the corpus. To repeat it without
them, cut a 2-minute 5.1 excerpt into six mono WAVs and time `subwoofa encode` against
`ffmpeg -i six_channel.wav -c:a <codec>`.
