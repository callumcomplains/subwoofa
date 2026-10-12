# Contributing to Subwoofa

Thanks for helping. Bug reports, test files (that you may share), format notes and code are all welcome.

## Reporting a problem

Open an [issue](../../issues/new/choose). Include the version (top left in the app, or the first line any command
prints), what you did, and what you expected. For a wrong or damaged stream, the output of
`subwoofa verify <file> --deep` helps a lot.

## Building

Rust (stable). On Windows the GNU toolchain needs no Visual Studio:

```sh
rustup default stable-x86_64-pc-windows-gnu
cargo build --workspace
cargo test --workspace --exclude subwoofa-app
```

The desktop app lives in `app/` (Tauri, plain HTML/CSS/JS in `app/ui`): `cd app && npm install && npx tauri build --debug --no-bundle`.

Tests that need FFmpeg (most round-trip tests decode the result with it) skip themselves when it is not on `PATH`,
so install it to run the full suite. Tests that need DTS or Dolby tools skip themselves when those are missing. If `SW_ASDCP_DIR` names a folder with `asdcp-info` and `asdcp-test` (built from [asdcplib](https://github.com/cinecert/asdcplib)), the DCP sound tests also check the files against them.

## How the code is organised

One crate per concern under `crates/`: `sw-core` (formats, layouts, versions), `sw-pcm` (reading input audio),
`sw-codec-*` (the encoders), `sw-verify`, `sw-tools` (StreamTools), `sw-queue`, `sw-cli`, and the app under `app/`.
`docs/FORMATS.md` lists what each format does and how it was checked.

## Code standard

New code follows the standard at the end of [docs/TODO.md](docs/TODO.md): a module comment saying what the file is
for and which specification it follows; short names; short functions; named, sourced constants; comments that explain
why; a test for each piece; and a round trip through an independent decoder where one exists.

## Legal

Please do not contribute anything copied from proprietary software or documentation, and do not attach files you
are not allowed to share. Subwoofa is licensed under the GPL (see [LICENSE](LICENSE)); by contributing you agree your
contribution is released under the same licence. See [docs/LEGAL.md](docs/LEGAL.md).
