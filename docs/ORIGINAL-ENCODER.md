# What the original encoder does, and how we know

Subwoofa's DTS streams are checked against the output of the original DTS-HD Master Audio Suite encoder (DTSHDMASE
2.x), the encoder that commercial discs are made with. This page records how that encoder is driven without its
window, what was measured, and what is still different. The scripts are in `tools/original-encoder-lab/`.

The encoder is used here only as a *reference* for how the format is written; no code or data of it is copied into
Subwoofa. Anything below was read back from streams it wrote.

## How the encoder is driven

The GUI is a Java front end (`DTSEncoder.jar`). For each job it

1. writes a job XML with every setting (Audio, Downmix and Bitstream tabs),
2. turns the XML into an encoder configuration file (`.cfg`, blocks `CHSETOBJ`, `LOSSY`, `LOSSLESS`, `LBR`,
   `PACKAGE`, `MIXCONFIG`, `MATRIX`, `FILEINFO`), using classes in the jar (`ConfigFileBuilder`),
3. sends one command line to the native engine `DtsJobQueue.exe` over `localhost:4444`.

The wire format is a four-digit zero-padded length followed by that many bytes of UTF-8. The command is

```
E E  "in1.wav" "in2.wav" ...  -r1 -s<fps> -t<tc start> -u<tc end> -v<encode from> -w<encode to>
     [-j<primary attenuation dB> -k<fade down s> -l<fade up s>] [-y | -y"branch points.csv"]
     -f"job.cfg" -o"<folder>\<name>"
```

with the input files in the order of the `CHANNEL` lines of the configuration. The engine writes `<name>.dtshd` (or
`.cpt` for DVD) and `<name>.dtshd_log.txt` ("Encode completed successfully"). It exits when its client disconnects,
so a batch keeps one connection open. A backslash followed by `t` or `n` in a path given to `-y` is read as an escape.

`Model.initialize(58)` selects the full Master Audio Suite product (57 is a reduced one that only offers DVD and
CD); with it the encoder's own configuration library lists every legal combination, which is
`docs/original-encoder-legal-settings.tsv` (339 combinations of destination, stream type, sample rate, layout and
bit rates).

## Bitstream tab

All of this is implemented (`crates/sw-codec-dts/src/mixmeta.rs`) and reproduces the original's output exactly in the
cases tested (`tools/original-encoder-lab/bs_compare.py`).

**Program information** goes into the `FILEINFO` chunk of the `.dtshd` file, not into the audio: `0x09`, the text in
ISO 8859-1 (characters it cannot hold become `?`), a newline, zero padding to a multiple of four bytes.

**Seamless branching** writes a `BRANCHPT` chunk after the blackout frame: the number of branch points (the start of
the clip always counts), the byte width of an entry (1, or 2 when a unit exceeds 255), the entries and zero padding to
four bytes. An entry is `floor(seconds * 48000 / 512) + 3`, in units of the 48 kHz core frame whatever the stream's
sample rate is. Points outside the encode are dropped (the log says so). The manual says the encoder disables "an
encoding filter" at a branch point; in the streams only the first few frames change (the core's inter-frame
prediction is off there). Subwoofa's core encoder does not use that prediction, so only the chunk is needed.

**Blu-ray secondary audio** (DTS Express, extension substream index 2) carries mixing metadata in every frame:

* the attenuation of the *primary* audio is a 6-bit code, new every frame. dB to code: half-dB steps from 61 (0 dB)
  to 31 (15 dB), one dB per code to 16 (30 dB), two dB per code to 1 (60 dB); the half-dB region starts each step
  0.125 dB early (thresholds 0.375, 0.875, ... 14.875), the others round to nearest.
* a fade is not linear in dB: the gain is `1 / (1 + (M - 1) u)^2` with `M = 10^(dB/40)` while `u` runs from 0 to 1
  over the fade. Fades start at the start and end at the end of the encode; the last frame is back at full level.
* a **mono** secondary also carries a mixing matrix that says which output speakers it feeds (C, L, R, Ls, Rs,
  LFE) and with what gain (same 6-bit code); stereo and 5.1 secondaries carry the attenuation code in the tail of
  the asset descriptor instead and are mapped to their own speakers.
* *Express Dialog Mode* turns on a half-band limit (`BAND_LIMIT_RATIO = 0.5`).

**Wide 7.1 remapping** adds six speaker-remapping sets to the asset descriptor of a 7.1 L R C LFE Ls Rs Lw Rw
stream (`exss::wide_remap_sets`). The speaker mask then needs 16 bits.

## Header sizes

The extension substream header is padded with zero bytes before its CRC to a fixed size per stream type, then to a
multiple of four if the content needs more: 32 bytes for Master Audio, 28 for High Resolution and Express (checked on
241 streams, no exception).

## Still different from the original (found by the sweep, to be worked through)

Sweep: every legal combination at its top bit rate was encoded by the original and by Subwoofa, and the first core
and extension headers were compared (`compare.py`). 80 of 339 are identical; the rest fall into these groups:

* layouts Subwoofa does not write yet: HRA 6.x discrete at 48 kHz (XCH plus XBR), HRA and 96/24 with 7.1 Cs/Ch,
  Cs/Oh, Lh/Rh, Lhs/Rhs and Lw/Rw, 96/24 core with 6.x, DTS CD at 88.2 kHz;
* DVD streams use 1509.75 kbps (frame 2013 bytes), not 1509;
* HRA at 96 kHz below 5.1 channels uses the same plain-core layout as 5.1; HRA at 48 kHz below 5.0 uses a
  non-perfect-reconstruction filter bank (FILTS 0) in the core;
* lossless-only (no core) Master Audio starts with two silent frames as the cored streams do;
* the 5.1 downmix flag of the asset descriptor is set in lossless 7.x streams;
* Express: the original codes the full band at every rate (header band byte 0x15) and fills the high bands
  parametrically; Subwoofa narrows the band instead (its waveform match is closer at low frequencies, the original's
  bandwidth is far larger); 5.1 secondary audio embeds a stereo downmix.
