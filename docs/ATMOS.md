# Dolby Atmos

Atmos has no public encoding specification. Subwoofa therefore does not encode it itself: it drives the **Dolby
Encoding Engine** (`dee.exe`), a program you obtain from Dolby and are licensed to use, then checks the result.

1. Install the Dolby Encoding Engine according to Dolby's terms.
2. Open **Settings > Engines > Dolby Encoding Engine**, choose `dee.exe` (double-click the box), or leave it
   on automatic if it sits in a standard location.
3. Open the **Atmos** tab, choose a master and an output format.

**Masters.** TrueHD Atmos and Dolby Digital Plus Atmos need a Dolby-authored master: a DAMF `.atmos` file or an
ADM BWF `.wav`. The engine refuses anything else. A plain channel bed (`.wav`, up to 7.1.4) can make Dolby Digital
Plus Atmos.

**Status.** Both are tested on Dolby-authored masters: a DAMF set from the Dolby Atmos Renderer and a 153-second ADM BWF from DaVinci Resolve. TrueHD Atmos comes out with four substreams and the 16-channel spatial flag, Digital Plus Atmos with Joint Object Coding on every access unit, and FFmpeg decodes both (the bed; it does not decode the objects). The engine writes nothing for a programme shorter than one frame of the output; Subwoofa reports that as an error.

Without the engine the Atmos tab is locked and says so.
