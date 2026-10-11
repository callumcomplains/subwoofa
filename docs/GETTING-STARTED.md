# Getting started

## Your first encode

1. Open Subwoofa. Pick a **Destination** (Generic is the relaxed one), a **Stream type** and a **Sample rate**.
2. Pick the **Channel layout** (5.1, 7.1 and so on).
3. Drop the left-channel file onto the first row. Subwoofa finds the other channels by name (`Mix_L.wav`,
   `Mix_R.wav`, `Mix_C.wav` ...). A single multichannel WAV works too: use **Import file...**.
4. Choose an output folder (double-click the box). The file name fills itself in, for example
   `SUBWOOFA - BD DTSHDMA 7.1 96kHz 24FPS`; type your own to take over.
5. Press **Encode**. The queue shows progress; each finished job is verified automatically.

Hover any **(i)** for what a setting does.

## The tabs

- **Inputs**: the files, an optional start and length, and the downmix and clipping preview.
- **Bitstream**: timecode, text stored in the file, seamless branching, commentary levels (DTS-HD).
- **Downmix**: embedded stereo mix and the 7.x to 5.1 and stereo levels (DTS-HD).

## Other modes

**StreamTools** edits existing streams, **Verify** checks them, **Atmos** encodes Dolby Atmos. Settings (the gear)
has the engine paths, themes and presets.

## What Subwoofa does not ship

FFmpeg, DTS software and the Dolby Encoding Engine are not included. See [ENGINES.md](ENGINES.md). Without them
the matching features are switched off, and the app tells you which and how to fix it.
