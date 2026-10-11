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


## Downmix tab

Driven with `batch13`-`batch17`, read with `xllhdr.py` / `xllu.py` (XLL channel-set headers) and `xxchparse.py` (the XXCH header), compared with `xlldiff2.py`, `xlldiff3.py` and `xxchdiff.py`.

* **Levels** are dB below full level with the table of the Bitstream tab's gains (`level_code`); in a Master Audio header they are indices into the 242-entry coefficient table (0.125 dB steps near 0 dB, 0.25 dB from 15 dB, 0.5 dB from 30 dB), in an XXCH header 6-bit codes (scale) and 7-bit codes (64 + code, coefficient). INF is "no coefficient" (0x100 / absent).
* **7.x to 5.1** (`use51Downmix`, default on for 7.x): each 5.1 channel has a primary level (default 3 dB), and the two extra channels of the layout (in speaker order) each have a level into every 5.1 channel (defaults: first extra into Ls, second into Rs, 3 dB). The pair does not matter: wide, height and centre-surround pairs fold into the surrounds. Master Audio: the header of the first set above the 5.1 base has one row per lower channel (scale = primary level, coefficients = the two extra levels); the core is the 5.1 channels scaled by the primaries with the extras added. High Resolution Audio: one scale for all (the Downmix tab refuses different primary levels there), the extras' targets as a mask plus a 7-bit code each.
* **Stereo** (`use20Downmix`): not embedded, the primary set's header carries `[present 1][embedded 1][type 3 bits: 1 Lo/Ro, 2 Lt/Rt][hierarchy 1]` and a 2 x N matrix (N = channels of the set, in speaker-bit order C L R Ls Rs LFE; row 0 to the left output, row 1 to the right). Not written for layouts with fewer than five channels or for 6.x with a centre surround that is not premixed.
* **Core auxiliary data**: with `use20Downmix` every 5.0 / 5.1 core frame (any stream type) sets AUXF and ends with `[audio][DSYNC][6-bit size = 4 + data + 2][zeros to a 32-bit boundary][sync 9A1105A0][ts 0 | dynamic downmix 1 | type 3 | 2 x N 9-bit codes, as in the lossless matrix, channels C L R Ls Rs LFE][zeros to a byte][CRC16 of the data]` (the original fills the audio to the last word and starts the block four bytes short of the end of the core part; the audio gets that much less). Not for 4.x and below, nor for ES. The embedded setting alone changes nothing in core, HRA or Express streams.
* **Embedded stereo** (`isEmbeddedDownmix`, Master Audio only; needs `use20Downmix`; not for 4.x and below): set 0 is `L R` (raw, not a residual of the core), set 1 the other 5.1 channels (a residual of the core, which is the plain 5.1 programme), then the extra sets. The header rows of set 1 have the scales `left20 a` / `right20 b` and the coefficients of C, Ls, Rs, LFE into the left and right output; cross terms (left to right) cannot be written. The embedded-stereo flag of the asset descriptor is set (and the embedded-5.1 flag as before when there are extras).
* **Making the stereo set**: the decoder undoes the sets from the bottom up, so the stereo channels are the original ones plus the 5.1 channels *as transmitted* (extras already folded in), each with the coefficient `D x inverse(set 1 row scale) x inverse(extra set row scale of the row) x (extra set row scale of the column)`. With a decoder's downmix modes (FFmpeg `-downmix stereo` and `5.1(side)`) Subwoofa's streams decode to exactly what the original's do, for every combination tried (random levels included).
* Not copied: the original sets the common header's "scalable resolution" bit (and adds fields to every channel set header) whenever a stream has downmix information; decoders do not need it and Subwoofa leaves it off. FFmpeg cannot open 7.1 Cs/Ch streams (also not the original's).


## DTS Express (LBR) in the original

Tools: `lbrchunks.py` (chunk list of a frame), `lbrts.py` (the time-sample chunk band by band), `lbrq.py` / `qcmp.py` (SNR by band against the sources); the decoder's grammar is FFmpeg's `dca_lbr.c`.

* Frame: 16-byte header, chunk 0x84, then chunks `[id (bit 7 set: 16-bit length)][length][data]`: 0x0A LFE; 0x30+p first grid (scale factors by group, third-grid averages), 0x40+p quantizer profile, LPC codes and the first two bands, 0x50+p bands 2 to 5 and LPC of band 2, 0x60+p the other bands, each as `[band index][third-grid scale factors the first time][samples]`; 0x0E / 0x1B..0x17 tones; 0x0B ECS (embedded channel set).
* **Bands are coded in a priority order** (the index of every band is in the stream), and the quantizer level belongs to the position, not to the band: the first six bands are fixed, the rest follow by falling energy. The profile byte is `steepness << 6 | overall level << 3 | strongest low band`; the original only ever uses steepness 0, with the overall level rising with the bit rate (mono 64 kbit/s: 3, 96: 4, 128: 4 to 5; stereo 96: 5, 256: 6; 5.1 192: 2 to 3, 510: 3 to 4). The chunk simply ends where the frame is full; samples of a truncated band, and bands that are not coded, are filled with noise by the decoder.
* Three-level bands use coding method 1 (a flag per sample and a sign for nonzero ones) when it is shorter than five samples in eight bits. Scale factors of a group are coded as points joined by interpolation over 1 to 7 slots (Subwoofa always uses distance 1). The second grid carries 64 attenuation steps (3 dB each, two samples) per group of bands from band 4 (bands 4 to 9, 10 to 17, 18 up), subtracted from the scale factor; a negative result silences the sample pair.
* The three lowest bands are linear-predicted: eight-tap synthesis `y[n] = r[n] - sum(c[j] y[n-1-j])` on the dequantized residual, reflection coefficients in four bits from `sin((i-8) pi / 17 or 15)`, two sets for bands 0 and 1 (samples 16 to 80 and 80 to 128), one for band 2; the first sixteen samples of a frame use the previous frame's last set. A closed loop over the decoder's output gains up to 7 dB on the lowest band.
* **5.1 secondary audio with embedded stereo** (not written by Subwoofa): header band byte +0x20, an extra channel pair (chunks 0x33 / 0x43 / 0x53 / 0x63) holding the stereo downmix, and an ECS chunk 0x0B (131 bytes at 192 kbit/s) of unknown content; FFmpeg does not decode it, so it cannot be tested.

## Still different from the original

Sweep: every legal combination at its top bit rate was encoded by both encoders and the first core and extension
headers compared (`compare.py`). Over 312 of 339 were identical in the last full run, and the fixes since then
(Express frame sizes, 96/24 ES, 6.x XCH, CD at 88.2 kHz) closed more. What is left:

* HRA 48 kHz 7.x: the original's XBR/XXCH split changes from frame to frame (a leaky reservoir); Subwoofa uses a
  fixed split fitted to its mean (about 0.27 x kbps + 58 bytes for XXCH).
* Blu-ray secondary audio, 5.1 Express: the original embeds a stereo downmix in the LBR stream (header band byte
  +0x20, EXSS embedded-stereo flag). Not written yet.
* High Resolution Audio has no embedded stereo downmix in the original either (the setting is ignored there).
* Express: Subwoofa now matches or beats the original's SNR at the same rate (mono 96 kbit/s is 1.6 dB behind) and keeps the narrower band at low rates; see above for what it does not do yet.
* Master Audio with 96 kHz extension: the EXTSS_MD chunk bit rates depend on content.
* Core quality differs in detail (Subwoofa has no ADPCM) but its SNR is equal or better in nearly all cases.
