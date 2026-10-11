# Original-encoder lab

Python and Java used to drive the original DTS-HD Master Audio Suite encoder without its window and to compare its
output with Subwoofa's. See `docs/ORIGINAL-ENCODER.md` for what it is for and what it found.

You need the encoder installed (it is not included), 32-bit Java 8 (its configuration library is a 32-bit DLL),
a JDK to compile the two Java files, Python 3 with numpy, and ffmpeg on the path. Paths are set at the top of
`lab.py`.

```
javac --release 8 -cp "<MAS dir>/DTSEncoder.jar;<MAS dir>/lib/*" -d java/classes java/com/dts/encoder/settings/*.java
```

Make ten-second mono stems (`stems/L.wav R C LFE Lss Rss Lsr Rsr Cs` at 96 kHz, 24 bit); other sample rates are
converted when first needed. Then:

* `lab.py` – `run_case(spec)` / `run_many(specs)`: one encode through the engine (keep one process, the engine quits
  when its connection closes).
* `sweep1.py max|min|all` – every legal combination; `compare.py <group>` – native against original headers;
  `qcmp.py <group>` – sizes and SNR; `bs_compare.py` – the Bitstream-tab references.
* `exssparse.py`, `chunks.py`, `aus.py`, `bindiff.py`, `hdiff.py`, `hdrpad.py` – readers for what comes out.
