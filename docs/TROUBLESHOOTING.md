# Troubleshooting

**"FFmpeg was not found" banner.** Install FFmpeg and point Subwoofa at it: [ENGINES.md](ENGINES.md). Native
encoders and WAV/AIFF inputs work without it.

**A stream type is greyed out and says "needs FFmpeg".** Same fix as above.

**A stream type says "(planned)".** It is listed so you can see what is coming; there is no encoder yet.

**Windows SmartScreen blocks the installer.** The app is unsigned. *More info*, then *Run anyway*. Compare the
file with `SHA256SUMS.txt` from the release if you want to be sure.

**Encode button is greyed out.** Read the log line under the button: it says what is missing (a channel file, the
output folder, a bit rate outside the allowed range, a sample-rate mismatch).

**Verify says a stream is damaged.** Run deep verify and open an issue with the report, if you can share it.

**Atmos tab is locked.** See [ATMOS.md](ATMOS.md).

**The app looks wrong in a custom theme.** Settings > Appearance > Custom, press **Reset to base**.

Still stuck? Open a [bug report](../../issues/new/choose) and include the version (shown top left of the app, and
printed by every CLI command).
