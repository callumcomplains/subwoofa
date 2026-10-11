# Security policy

Subwoofa is a desktop and command-line tool that reads audio and stream files you choose and writes files you
choose. It has no server, account or network component. The only time it starts another program is when you
configure one (FFmpeg, the DTS verifier, the Dolby Encoding Engine), and the paths come from your own settings.

## Reporting a vulnerability

Please report privately: on the repository's **Security** tab choose **Report a vulnerability** (GitHub private
advisory). Do not open a public issue for something exploitable, for example a crafted stream that crashes or
corrupts memory in the reader or verifier.

Include the version (top left in the app, or the first line any CLI command prints), the file if you can share it,
and the steps. This is a spare-time project: expect a first answer within about a week.

## Supported versions

Only the latest release.

## Downloads

Releases are built by GitHub Actions from a tagged commit and come with `SHA256SUMS.txt`. They are not code-signed
yet, so Windows SmartScreen will warn; compare the checksum before running.
