# Subwoofa: working notes for Claude

Read this whole file before doing anything. It was written at the end of a 270-commit session (2026-10-16, repo at
commit `28cdfcd` and later) so a fresh session can carry on with no loss. It records who the user is, every standing
instruction, how the project is built and checked, what is done, what is open, and every trap that cost time.

If something here disagrees with the code, the code wins: check, then fix this file.

---

## 0. The rules that matter most (read these twice)

1. **You write all the code; the user is not a coder.** Explain things in plain language, briefly. The user verifies
   outputs (listens, plays discs, tests installers) and makes the release decisions. Never assume they read code.
   They have said "sometimes I look at your output and go wtf am I even looking at".
2. **Never tag a release, create a GitHub release, or change repository visibility.** The user does those. (The repo is
   now public and `b0.1.0-2026-10-11-18-28-28` is already published as a pre-release.) Pushing commits to `main` is
   normal and expected; do it after the checks in section 9 pass.
3. **Never print, copy, save or repeat the Dolby Encoding Engine's licence feature line** (the `Features enabled by
   the license: ...` log line; it names the licensee). The backend filters it. `dee.exe --print-stages` prints it
   unfiltered: do not run that without filtering, and never quote it. Never ship, copy or log the engine itself.
4. **Nothing third-party is shipped.** No FFmpeg, DTS or Dolby software, logos or bitmaps in the repo or releases.
   The FFmpeg download is a user-requested fetch onto their machine (installer checkbox, Settings button), not shipping.
5. **Do not forge Dolby-authored masters** to get past the engine's "not authored with Dolby tools" refusal. Masters
   come from Dolby tools (DaVinci Resolve Studio's ADM BWF export, the Dolby Atmos Renderer's DAMF).
6. **Do not drive the original DTS encoder's GUI** (`dtshd.exe`). Everything for the original encoder goes through its
   engine socket (section 6.2). The user authorised full headless use of it.
7. **Run clippy, the full tests and the golden checksum test before every push** (section 9). Two bad pushes happened
   by skipping this (a doubled `w.put(0xFFFF,16)` in XBR; a mangled `release.yml`).
8. **Never capture the whole screen.** Once a full-screen capture showed the user's desktop (browser tabs). Screenshots
   of the app go through the browser-debugging port of the app window only (section 8.4).
9. **Do not drive the user's computer in ways that interrupt them** (key presses to other windows, opening menus). An
   `AppActivate/SendKeys` attempt opened their Start menu once.
10. **Use the Write tool (or Edit) for any file or script containing apostrophes, backslashes or `$`.** Bash heredocs
    and `perl -pe` replacements corrupted files several times (section 12). Python is not on PATH in Git Bash
    (`python3` missing); use Node (`/e/Tools/node-v24.21.0-win-x64`) for scripted edits.
11. **Kill `subwoofa-app.exe` before any cargo build or clippy** (it locks files: "os error 32"). `taskkill //F //IM subwoofa-app.exe`.
12. **Commit attribution:** every commit message ends with `Co-Authored-By: Claude Sonnet 5.5 <noreply@anthropic.com>`
    (the harness reminder says so; follow the latest reminder if it changes). PR bodies end with the Claude Code line.
13. **Git identity is already configured in the repo** (`callumcomplains`, noreply email `45427259+callumcomplains@users.noreply.github.com`).
    A non-noreply email makes GitHub reject the push. Do not change it.

---

## 1. The user and how to work with them

- GitHub: `callumcomplains` (renamed from `CallumUncensored` on 2026-10-12). Email in harness context is for
  identification only, never send it anywhere. Windows 11 laptop/desktop; drive letters C: (nearly full, ~22 GB free,
  install nothing there), E: (repo, tools), F: (test outputs), O: (media library).
- Not a coder; had an idea and had Claude build it. The README says so openly ("Hi, Callum here... All of the code base is
  from Claude... run it through Claude Code's Sonnet 5.5 Medium"). Keep that honesty; do not remove or soften it.
- Works long unattended: often says "I'm going to sleep / work" and expects steady autonomous progress, resuming after usage
  limits reset. Prefers **doing** over asking. Asks "anything else you can churn on?" and expects a real list.
- Tests the installer in **Windows Sandbox** (fresh image, no WebView2, so the WebView2 step is slow there; real PCs
  usually have it). Expects files delivered with `SendUserFile` when too big for chat say so (30 MB limit).
- Wants short plain explanations, and honesty about what was and was not tested. They notice when a claim is wrong
  (e.g. my early claim that DEE's `edit_ddp` edits metadata was wrong: it trims/joins; I corrected it).
- Likes small humour in UI tooltips but "keep things simple". Deferred: "Some strings still need AI-desloping, defer this."
- Said code signing is something to do **together** ("we can look into code-signing together").
- Asked whether an Opus review pass over the code would help; the answer given was yes, as a findings-only review of
  the byte-exact encoders, `sw-verify`, loudness/compare code and Tauri file handling; **not yet decided or run**.
- Reports show I should always say: what I did, what I tested, what I did not test, what is open, what needs them.

### Everything the user has asked for (chronological, condensed)

Origin and release
- An open-source replacement for the abandoned DTS-HD Master Audio Suite Encoder (DTSHDMASE), covering all DTS and
  Dolby multichannel formats (+ audio files), GUI + CLI, Atmos via a Dolby Encoding Engine they supply.
- Public beta: GPL-3.0-or-later, version `b0.1.0 (UTC stamp)`, tag `b0.1.0-YYYY-MM-DD-HH-MM-SS` (user tags), CI +
  release workflows, unsigned Windows installer, docs in `docs/`, lab material in the private repo `subwoofa-lab`.
  The user turned on GitHub vulnerability reporting, made the repo public, and published the first pre-release.
- NHK 22.2 input layout added to the roadmap (done). Reference material they supplied: Oppenheimer, Terrifier 3, Dire
  Straits, Lady Gaga, Deep Purple disc, City of God stems, Shrek DTS:X, Tokyo 2020 22.2 (see section 8.2).

GUI
- Look: "studio utility": inspired by the Dolby Atmos Renderer, Ableton Live and DTSHDMASE; dense, amber on dark. Ten
  themes plus a custom theme editor. Wanted: explanations only inside **(i)** tooltips (deslop the text, a touch of
  humour, keep it simple), nothing clipped (a text that does not fit its box is a bug), path boxes open their dialog on
  **double-click**, custom FFmpeg and DTS-HD MAS paths in Settings, Settings as a full page with tabs, (i) tooltips must
  show above the settings page, a larger minimum window (now 1200x780), "make it feel premium".
- Later: installer with logos / customised, an **optional FFmpeg download in the installer**, **add Subwoofa to PATH**,
  and a **new installer page with checkboxes** (done: the Extras page).

Quality and process
- "Write all new code readable so it never needs a rewrite" (standard in `docs/TODO.md`; names short, test names
  at most 2-3 words). The big readability rounds (three of them) were done; see section 11.
- Speed round: as close to FFmpeg as possible or faster, options for cores and fast/slow with a file-size warning (done).
- Work through the TODO list unattended, keep a running changelog, continue after limits reset.
- "Churn on Speed and Readability, and then jobs a gooden."

Features requested after "what would you add?" (roadmap in `docs/TODO.md`)
- Loudness built in (done), a real QC report with compare-with-source (done), preview with A/B (done), automation
  (JSON jobs, watch folder: done; headless server open), more inputs, metadata edits, downmix design, reach.
- "Start on 1 and 2" (done), then continue through the list; "Continue", "Keep continuing", "Continue on everything that's
  still open" were their repeated instructions in the last hours.

DEE (Dolby Encoding Engine)
- "Have we finished exploring DEE features? Is there anything we haven't implemented?" then "Work on wiring everything
  up!" (done for everything the installed engine build supports; section 6.3).

Delivery requests in the last hours
- "Send over the installer, I'll reset my Windows Sandbox", "compile newest version and output to working folder"
  (`dist/`), "attach setup file to this chat". The release build takes ~6-8 min; `dist/` is git-ignored.

---

## 2. What the project is

**Subwoofa** is an open-source (GPL-3.0-or-later) multichannel audio encoder with a Tauri GUI and a CLI. Give it mono
channel files (or one multichannel file) and it writes DTS (core, ES, 96/24, HD High Resolution, HD Master Audio,
Express), Dolby (AC-3, E-AC-3, TrueHD, MLP; Atmos through the user's own Dolby Encoding Engine) and audio files (WAV
family, AIFF, CAF, FLAC, Ogg FLAC, ALAC, TTA, WavPack, Monkey's, ALS, DSD, DCP MXF). It verifies what it writes, has
StreamTools (trim/split/join/replace/insert/re-stripe/silence/dialnorm), loudness, QC reports and previews.

- Repo (public): https://github.com/callumcomplains/subwoofa, branch `main`, local `E:\FormatProgram\subwoofa`.
- Private research repo: `callumcomplains/subwoofa-lab`, local `E:\FormatProgram\subwoofa-lab` (scripts that drive the
  original encoder, bit-level comparison tools, effort/speed harnesses, `docs/ORIGINAL-ENCODER.md`,
  `docs/DEVELOPMENT-LOG.md`). Paths inside point at this machine.
- Other folders: `E:\FormatProgram\TEST FILES` (reference material, section 8.2), `E:\FormatProgram\_stash\sw-adm-stub`
  (an unowned empty ADM-reader stub that appeared once in the repo; moved out; ignore), `E:\FormatProgram\SCRIPT` (user's own
  audio scan scripts, not ours), `subwoofa-backup-before-rename.bundle` (git bundle backup from the GitHub rename).
- Size: ~50k lines of Rust across 13 crates + `app/src-tauri`, ~3.3k lines of page script/HTML/CSS. ~330 tests.
- Status at the end of this session: **271 commits**; CI green; version `0.1.0` displayed as `b0.1.0 (dev build)` locally
  (the stamp comes from `SUBWOOFA_BUILD_STAMP`, set only by the release workflow); one published pre-release
  (2026-10-11), **older than the current code**: the next tag will carry everything in `CHANGELOG.md` > Unreleased.

---

## 3. Architecture

```
 stems (WAV, AIFF, ...)        app (Tauri, app/ui)           CLI (sw-cli)
        |                           |                            |
        v                           +------- EncodeSpec ---------+     (JSON job files are EncodeSpecs too)
   sw-pcm: read as f32                         |
        |                                      v
        |                           sw-queue::run::execute  (log, progress, verify, engine choice)
        v                                      |
   sw-codec-*  <---- sw-core (formats, layouts, limits) ---- sw-backend-ffmpeg / sw-backend-dee
        |
        v
   .ac3 .eac3 .thd .mlp .dts .dtshd .flac .wv ...  --->  sw-verify (read back, deep decode, compare with source)
```

### 3.1 Crates (lines of Rust, what each owns)

| Crate | Lines | Owns |
|---|---|---|
| `sw-core` | 2.1k | Vocabulary: channel layouts and IDs (30 layouts, `ChannelLayout::find`), the destination/stream/rate/bit-rate capability matrix (`formats.rs`: `StreamType`, `Destination`, `validate`, `bitrate_range_kbps`), Bitstream-tab settings (`bitstream.rs`), downmix matrices (`downmix.rs`), `Effort` (fast/balanced/best), `autoimport` (finding sibling channel files, `scan_folder`), version string (`version.rs`, reads `SUBWOOFA_BUILD_STAMP`). |
| `sw-pcm` | 1.1k | Input audio: WAV/RF64/AIFF streaming reader, `channels` (which file channel feeds which speaker; mono files, stereo pairs, multichannel files with order policy auto/mask/file), `StemSet` (per-channel files read together; `select_span`, `read_ahead`, `read_planar`, `set_downmix`, `set_gain_db`, `set_delay_frames`). |
| `sw-loudness` | 0.5k | BS.1770-4/EBU R128: K-weighting, gated blocks, LRA, true peak (4x/2x polyphase), channel weights; `Meter`, `Loudness`. |
| `sw-codec-ac3` | 4.0k | AC-3 and E-AC-3 (1.0-5.1, E-AC-3 also 7.x via dependent substream; block switching, 2/0 rematrixing, coupling), plus `edit.rs` (change dialnorm without re-encoding, fixes CRC1/CRC2). |
| `sw-codec-truehd` | 3.9k | TrueHD and MLP lossless: FIR+IIR predictors, cross-channel matrices, stereo-downmix substream with LSB bypass, `analyze.rs` stream analyser. |
| `sw-codec-dts` | 15.9k | Core (`dca/`), ES, 96/24 (X96), HRA (`hra/`: XBR, XXCH, hra96), Master Audio (`lossless/`: XLL, ma, ma_core, embedded downmixes), Express/LBR (`express/`), container (`exss`, `.dtshd`, mixmeta). The biggest crate. |
| `sw-codec-pcm` | 9.0k | Audio-file writers behind one trait `BlockWriter`: wav, aiff, caf, flac, ogg, alac, tta, wavpack/, monkeys/, als/, dsd/, dcp/. |
| `sw-backend-ffmpeg` | 0.6k | Running FFmpeg (`find_ffmpeg`, override path, `downloaded_dir()` = `%LOCALAPPDATA%\Subwoofa\ffmpeg`), joins inputs with explicit `map=`. |
| `sw-backend-dee` | 2.6k | Dolby Encoding Engine: Atmos jobs (`AtmosJob`, `profile.rs`, `alias.rs`, `run.rs`) and everyday tasks (`task.rs` = `TaskJob`/`Task`/`Source`/`Measured`, `taskrun.rs` = staging + run). |
| `sw-verify` | 8.0k | Verification per stream type (ac3, thd, dts/, audiofile/), deep mode (`deep/`: FFmpeg full decode, `AudioTap` loudness + spectrograms), HTML QC report (`html.rs`), `analysis/`, `source.rs` (compare with source: lag by FFT cross-correlation, channel matching, bit-exact or SNR). Optional DTS verifier as second opinion. |
| `sw-tools` | 1.4k | StreamTools for DTS/DTS-HD/TrueHD/MLP: index, cut, join, container, timecode, silence, check. |
| `sw-queue` | 3.0k | Job queue (`lib.rs`), job runner `run/` (`spec`, `sources`, `encoders`, `log`, `mod`), `measure.rs`, `preview.rs`, `dolby.rs`, `threads.rs`. |
| `sw-cli` | 2.3k | `subwoofa` command: `args`, `encode`, `verify`, `tools`, `atmos`, `dolby`, `downmix`, `listing`, `loudness`, `jobs` (save/load/run), `watch`, `progress`, `main`. |
| `app/src-tauri` | 1.3k | Tauri bridge: `catalog`, `sources` (incl. `path_stats`), `jobs`, `downmix`, `loudness`, `preview`, `dolby`, `streamtools`, `toolpaths` (incl. `fetch_ffmpeg`), `atmos`. |

`docs/ARCHITECTURE.md` is the maintained map; keep it current when adding modules.

### 3.2 The job spec (`sw_queue::run::EncodeSpec`)

Plain serde data used everywhere (app, CLI flags, preset files, JSON job files, preview, watch). Fields: `dest`, `stream`,
`layout`, `sample_rate`, `bitrate`, `dialnorm`, `start`, `duration`, `inputs` (channel id -> path), `output`, `unlocked`,
`engine` (`""`/`ffmpeg`/`native`/`dolby`), `downmix` (TrueHD stereo substream), `channel_order`, `es_phase_shift`,
`rear_minus_3db`, `source_layout` (22.2 mixdown), `core_x96`, `pcm_format`, `bitstream` (Bitstream tab), `speed`,
`threads`, `normalize_lufs`, `delay_ms`, `write_log` (app yes, CLI no). Adding a field means updating every struct literal
(CLI `encode.rs`, tests `compare.rs`, `golden.rs`, `queue.rs`) and giving it a serde default.

### 3.3 How a job runs (`run/mod.rs::execute_inner`)

validate -> `open_sources` (StemSet; `source_layout` mixdown; rate check) -> `set_delay_frames` -> span (`span_of`) ->
log header -> `choose_engine` (native vs FFmpeg vs dolby; delay/normalise force native, error for non-native formats
or the dolby engine) -> optional `normalise` (measure span, gain, true-peak cap -1 dBTP) -> build `ff::Job` ->
`run_encoder` (audio file / TrueHD / Express / MA / DTS core / AC-3 / FFmpeg / `encode_dolby`) -> `log_result` ->
verification (`verify_output`; skipped for downmixed sources or when no FFmpeg for non-audio files).

### 3.4 App (`app/ui`, plain HTML/CSS/JS, no bundler)

Scripts, in load order from `index.html`: `core`, `layout`, `downmix`, `job`, `preview`, `watch`, `bitstream`, `themes`,
`tabs` (the `INFO` list = every (i) tooltip, `showTab`), `presets`, `start` (init), `streamtools`, `dolby`, `verify`,
`settings`, `atmos`, `main` (starts it; order matters: `init()` is async, `initTools/initVerify/initAtmos/initDolby` run
before the catalogue loads, so anything needing `cat` must be filled inside `init()` in `start.js`).
Modes: Encode (tabs Inputs / Bitstream / Downmix), StreamTools (ops Trim, Split, Append, Replace, Re-stripe, Add silence,
Dialnorm, Dolby engine, Verify), Verify (deep, Save report, Compare with source), Atmos, Settings page (General,
Performance, Engines, Appearance, Presets). Footer: output folder + name + Speed + Preview + Encode; Preview panel
(source/encoded/difference, loop, blind mode, excerpt start/length); queue list. Ten themes (Studio Amber default,
Graphite, Midnight, Ocean, Moss, Rose Dusk, Terminal, Paper, Daylight, High Contrast) + custom (six colours). Format
plates are plain-text SVGs in `app/ui/logos/` (no third-party logos, by LEGAL).
Window: 1320x860, min 1200x780, `withGlobalTauri`, a CSP (`self` only, inline styles allowed, IPC; Tauri hashes the inline
theme script itself; `dangerousDisableAssetCspModification: ["style-src"]` keeps `'unsafe-inline'` working). Put
page-built HTML through `esc()` (`core.js`).

Tauri commands (registered in `main.rs`): catalog, options, auto_import, multichannel_pick, scan_folder, path_stats, probe,
downmix_default/safe/check, queue_add/list/cancel/remove/clear/set_concurrency, read_log, measure_loudness,
set_dee_path, measure_loudness_dolby, dolby_run, dolby_cancel, make_preview, preview_cancel, read_preview_file,
write_text, set_tool_paths, tool_status, fetch_ffmpeg, tools_* (info, pbr, trim, split, append, replace, stripe,
restripe, add_silence), ac3_dialnorm_info, ac3_set_dialnorm, verify_file (with optional `against`), atmos_locate,
atmos_inspect, atmos_status, atmos_cancel, atmos_start. Events: `queue`, `preview`, `dolby`.

---

## 4. Formats and their state (full table: `docs/FORMATS.md`)

All native encoders were written from public specs (ETSI TS 102 114 DTS, TS 102 366 AC-3/E-AC-3, Dolby's TrueHD
high-level description, community MLP notes) and by reading what real streams contain. "Verified" = decoded by FFmpeg
(bit-exact for lossless) and, for DTS, `DTSHDVerify.exe` (a strong oracle; use it for every DTS-HD output).

- **AC-3 / E-AC-3**: 1.0-5.1 (+7.x E-AC-3), any E-AC-3 rate 8-1024 kbps, block switching, rematrix, coupling; ~50-190x
  real time. Dolby's decoder and FFmpeg agree on our streams (AC-3 to ~66 dB, E-AC-3 to ~40 dB; both equally far from
  the source). **Not built: E-AC-3 enhanced coupling** (open).
- **TrueHD / MLP**: up to 7.1, 44.1-192 kHz, real stereo-downmix substream, bit-exact; 2.4% smaller than FFmpeg's;
  stereo 0.05% above the commercial disc's; 5.1 now 0.7-1.4% above Dolby's own encoder (section 7).
- **DTS**: core (<=5.1, no ADPCM/VQ), ES matrix/discrete (XCH), 96/24 (X96), HRA (XBR 5.1/48k; XXCH 7.x; core+XXCH+X96
  7.x/96k; commercial layouts), **HD MA** (CBR core + XLL, 1.0-7.1 at 44.1-96 kHz, 176.4/192 up to 5.1 with core,
  immersive layouts up to 22.2 as core-less hierarchical sets, embedded downmixes; ~5% smaller than DTSHDMASE on a full
  film), **Express/LBR** (1.0-5.1, tonal components in, quality at/above the original at equal rate), DTS Music Disc
  (DTS-in-PCM WAV). Bitstream tab (program info -> FILEINFO chunk, BRANCHPT seamless branching CSV, secondary-audio mix
  metadata with fade law, wide-7.1 remap, EXSS padding rule) and Downmix tab (embedded stereo, 7.x->5.1) verified exact
  against the original encoder (~312+/339 headers identical in the sweep).
- **Audio files**: WAV/BWF/RF64 (time reference), AIFF/AIFF-C, CAF, FLAC (multichannel, Ogg), ALAC, TTA, WavPack, Monkey's
  (mono/stereo), MPEG-4 ALS (no long-term prediction/BGMC), DSD64/128 (DSF/DFF; checked by tone SNR), DCP sound MXF
  (SMPTE/Interop; **not tried on a cinema server**).
- **Atmos**: TrueHD Atmos and DD+ Atmos through the user's DEE (section 6.3); both **now verified on real Dolby-authored
  masters** (DAMF stub, a 153 s DaVinci ADM BWF). Detection of DTS:X in Verify; **DTS:X encoding not possible** (needs
  DTS's licensed encoder). Verify reads JOC in E-AC-3.
- Layouts: 30, incl. 22.2 (input; mixed down by panning with `--source-layout 22.2`), 9.1.6, 7.1.4...
- **AC-4** (checked 2026-10-16): the installed engine has templates but no AC-4 filter ("Unknown filter
  (encode_to_ims_ac4)"); FFmpeg 8.1 has no AC-4 decoder (raw demux/mux only). Needs an engine build with IMS AC-4.
- **Not done / parked**: HRA 6.1 at 48 kHz (needs a reference), DVD-Audio disc structure (AOB packs), Atmos Theatrical (IAB
  MXF), DTS:X encode, AAF import, AC-4 (DEE build has no AC-4/AAC encoder), MPEG-H, Auro-3D, Datasat.
- Known quality limits: Express quality "modest" in README known-limits; TrueHD analyser cannot follow substream 2 of
  Dolby's Atmos TrueHD (warning only); MA 192 kHz band 1 is 3.4% larger than the original's (investigated, not located).

---

## 5. Feature inventory (what exists beyond encoding)

- **Loudness** (`sw-loudness`, `sw-queue::measure`): `subwoofa loudness [--json] [--dolby]`; `encode --auto-dialnorm
  [--dolby-meter]`; app Measure button (+ "Dolby meter" checkbox when DEE found) sets dialnorm and flags true peak > -1
  dBTP. Checked against FFmpeg `ebur128`. Caveat documented: whole-programme loudness is a stand-in for dialogue level;
  Dolby's meter finds the dialogue.
- **Normalise**: `--normalize LUFS` / app row: gain so integrated loudness hits the target, true peak capped at -1 dBTP;
  needs a native-encoder format (error otherwise, also with the dolby engine).
- **Delay**: `--delay-ms N` / app Delay field: silence in front, or start left out when negative. Same native-only rule.
- **QC report**: `verify FILE --report out.html` (self-contained HTML: verdict, loudness, per-channel peaks, bit-rate chart,
  canvas spectrograms from base64 level bytes, collapsible sections); `verify FILE --against SOURCE [--layout L]
  [--source-start/--source-duration]` (lined-up decode; exact proof for lossless, per-channel SNR for lossy; channels
  with RMS below -60 dBFS excluded from lossy SNR).
- **Preview** (`sw-queue::preview`): encodes an excerpt (default 20 s from the middle; chosen start; 10-60 s) with the
  job's exact settings, decodes with FFmpeg, folds source and decode to stereo identically (LFE left out), aligns by
  correlation, writes three 16-bit WAVs (source, encoded, difference) to `%TEMP%\subwoofa-preview`, reports kbps and SNR.
  Player uses Web Audio, keys 1/2/3/space, blind A/B quiz mode.
- **Automation**: `encode --save-job FILE`, `subwoofa run JOBS.json [--keep-going] [--verify]`, `subwoofa watch FOLDER --job
  FILE --out DIR [--once] [--move-done] [--interval N] [--verify]` (stability fingerprint, hidden `.name.partial.ext`
  temp, skips sets whose output exists, failed sets not retried until restart); app "Watch folder…" button (JS poller using
  `scan_folder` + `path_stats`; **sort the file list before fingerprinting**: HashMap order is random).
- **StreamTools**: CLI `tools info|pbr|trim|split|append|replace|insert|restripe|add-silence|dialnorm|verify`; the app
  tabs mirror them. Dialnorm edit rewrites only the 5-bit field(s) (AC-3 incl. dual-mono `dialnorm2`, E-AC-3) and CRCs.
- **Dolby engine extras** (section 6.3): Dolby meter, Dolby's encoders (`--engine dolby` for ac3/eac3/truehd), `subwoofa
  dolby decode|transcode|edit|correct`, StreamTools "Dolby engine" tab, Atmos trims and dialogue detection options.
- **Other**: presets (save/load), folder batch (`Batch from folder…`), queue concurrency, auto-verify, per-job log
  `<name>_log.txt`, file-name auto-naming (`SUBWOOFA - BD DTSHDMA 7.1 96kHz 24FPS`), unlocked mode.

---

## 6. External tools and engines

### 6.1 FFmpeg
Not shipped. Found via override path (Settings) > PATH > `%LOCALAPPDATA%\Subwoofa\ffmpeg\ffmpeg.exe`. Local copy:
`C:\ProgramData\chocolatey\bin\ffmpeg` (8.1 essentials, gyan.dev). Used for reading non-WAV/AIFF inputs, formats with no
native encoder, **decoding every verification**, preview. FFmpeg quirks learned: `join` maps by channel names embedded in
files (always pass `map=`); `join` can drop the final partial frame (pad with `apad` and trim to exact frames); the
`sine` source defaults to amplitude 0.125; the truehd decoder has `-downmix <layout>` (decode only the stereo
presentation: `ffmpeg -downmix stereo -i x.thd ...`; `-request_channel_layout` no longer exists); its AC-3 decoder applies
the stream's dynamic-range words by default (differences vs Dolby's decode with DRC none).
The installer/Settings fetch: `app/src-tauri/installer/fetch-ffmpeg.ps1` downloads gyan.dev's current "essentials" zip
(~115 MB), verifies the published SHA-256 (the `.sha256` URL returns bytes, read it as text), runs the result once, keeps
`ffmpeg.exe`/`ffprobe.exe` in `%LOCALAPPDATA%\Subwoofa\ffmpeg`, logs to `%TEMP%\subwoofa-ffmpeg-log.txt`.

### 6.2 The original DTS-HD Master Audio Suite (DTSHDMASE) = the reference encoder
Installed at `C:\Program Files (x86)\DTS\MAS-SAS` (`DtsJobQueue.exe`, `DTSEncoder.jar`, `DTSHDVerify.exe`, ...). Driven
headlessly: start `DtsJobQueue.exe` with cwd there (takes ~5 s to listen on `localhost:4444`); framing = 4-digit
zero-padded ASCII length + UTF-8 text; it **quits when the client disconnects**, so use ONE long-lived socket per batch;
command `E E "in1.wav" ... -r1 -s<fps> -t<tc> -u<tc> -v<from> -w<to> [-j<att dB> -k<fade down> -l<fade up>] -f"job.cfg"
-o"outdir\name"`; the cfg is produced by the encoder's own Java (`GenCfg.java`, 32-bit JRE8 at `C:\Program Files
(x86)\Eclipse Adoptium\jre-8.0.472.8-hotspot`, `Model.initialize(58)` = full product). All of it, plus the 339-combination
sweep (`lab/legal_all.tsv`), is in the **private lab repo** and `docs/ORIGINAL-ENCODER.md` there. The user authorised full
headless use. `DTSHDVerify.exe` is the oracle for DTS-HD output.
Remaining differences recorded in the lab doc (HRA 48k 7.x dynamic XBR/XXCH split, core96 X96 size varies, BD secondary
5.1 embedded stereo ECS chunk 0x0b unknown, 6.xd Cs set is residual of core in the original, DVD 1509.75 frame size,
HRA <5ch, CD 88.2k...). `docs/REFERENCE-ENCODES.md` lists the 13 Bitstream-tab reference encodes and other references
still wanted (only the unblocked-by-user ones remain).

### 6.3 Dolby Encoding Engine (DEE 5.2.1)
At `C:\Program Files\Dolby Encoding Engine\dee.exe` (licence from the user's lecturer via a Dolby rep; **never ship, copy or
print the licence line**). Chosen path is held by `sw_backend_dee::set_dee_override` (the app calls `set_dee_path`);
`find_dee(None)` also honours env `SUBWOOFA_DEE` and the standard install folder.
**Quirks (all paid for in debugging):**
- `file_name` and storage folders are **split on spaces**: paths with spaces fail. Atmos masters get an alias in a space-free
  folder (`alias.rs`); a DAMF `.atmos` is a text listing naming `metadata:` and `audio:` companions, so the alias copy is
  rewritten (`retarget_damf`). Tasks stage inputs/outputs as hard links/copies in `work_folder()` (`%TEMP%\subwoofa-dee`, a
  space-free folder is required; falls back to `%PUBLIC%`/`%ProgramData%`).
- The engine refuses masters not authored by Dolby tools. Masters go in as `atmos_mezz` input; TrueHD Atmos needs a master
  (a 12-ch WAV is refused); DD+ Atmos works from a 7.1.4 WAV bed. Filter schema versions: atmos ddp v3, others v1, convert v2.
- It can exit 0 and write a **0-byte file** for a programme shorter than one frame (the 512-sample DAMF test stubs): both
  runners now treat empty output as an error.
- `encode_to_dthd` on plain WAV/wav_list input makes ordinary lossless TrueHD, **but its 6/8-channel presentations lower the
  surrounds 3 dB by default**; Subwoofa sets `surround_3db_attenuation=false`. `pcm_to_ddp` surround phase shift/3 dB
  also default on in the engine's templates; our defaults are off (faithful encode of a finished mix).
- Loudness: `measure_loudness` with `dialogue_intelligence` gives speech-gated integrated loudness; manifest-only output
  works; `ddp_decode` and `pcm_loudness_correction` need the manifest output connected (else "does not connect").
- `edit_ddp` = **trim/join E-AC-3**, not metadata edit. `pcm_channel_order` refuses content under 20 minutes. This build has
  **no AAC and no AC-4 filter** ("Unknown filter (encode_to_aac)"); templates for them exist but do not run. Video (Dolby
  Vision/HEVC) is out of scope.
- The engine writes `<output>.log` (rich: downmix limiter, matrices, per-channel max values) and `.mll` beside outputs.
  `dee.exe --schema FILE` writes the full XSD (420 kB; use `node` to list a filter's options); `xml_templates/` holds example
  jobs for every filter. `--print-stages` prints the licence line (do not).
- Exposed in Subwoofa: Atmos (TrueHD: DRC, clusters 12/14/16, legacy authoring, 8/6/2ch presentations, optimise data rate,
  custom dialnorm, metering + dialogue switch + speech threshold; DD+ Atmos: rate, DRC, downmix levels, preferred downmix, 5.1
  surround/height trims), convert, measure, DD/DD+ encode (dd, ddp, ddp71; DD only measures loudness), TrueHD encode,
  decode, transcode, edit, correct. Not exposed: timecode base, embedded timecodes, log format.

---

## 7. Deep notes on the hardest codec work

### TrueHD (what was learned in the last stretch)
- Stream shape for 5.1/7.1: **substream 0** carries the 2 coded channels of the stereo presentation (coded channels 0,1),
  **substream 1** the other channels, with primitive (lifting) matrices that rebuild everything. Our downmix: `Lo = (L*2^14 +
  sum lo_j x_j)/D` with `D = 32767` (-6 dB) needs one **LSB-bypass bit** per sample per front channel (4.3% of the stream).
- **Decoder semantics learned:** in a full decode the later substream's matrices start from the *coded* values of channels
  0/1 (not the earlier substream's post-matrix output); that is why a prediction matrix for channel 1 appears in **both**
  substreams, and why prediction on the stereo pair is lossless for the 6-ch decode while substream 0 alone still yields the
  presentation. Verified by FFmpeg full decode (exact) and `-downmix stereo` (byte-identical presentation).
- **The fix that closed 3%:** `analyse()` forbade cross-channel prediction for the stereo pair in the non-"unity" path;
  allowing it made 5.1 files 2.2-3.2% smaller, still exact. Remaining gap to Dolby's own encoder: 0.7-1.4%. "Unity mode" for
  24-bit sources (code the full-level pair, scale by ds0 matrix) was tried: exact but no smaller, reverted.
- Dolby's file uses IIR order 1-3 on most channels, FIR orders 5-8, coarse FIR shift 8-10, non-zero Huffman offsets and
  six-term matrices; ours is almost all order-8 FIR at shift 13-15 with IIR order 0 (Best uses fixed-IIR/MA tools: costly
  for <=0.6%). A better predictor search (ARMA fit) is the likely remaining lever. Use `subwoofa stream-info` on both files.
- Decorrelation is off at Fast effort. Max 8 primitive matrices per substream (budget logic in `analyse`).
- Test recipe: `subwoofa encode --stream truehd --dest generic --layout 5.1 --sample-rate 48000 --engine native --speed best
  ...` vs `--engine dolby`; compare byte sizes per second of audio (Dolby trims ~3072 frames at the end), verify with
  `verify FILE --against SRC --layout 5.1` (exact).

### Verifier/analyser gap
Our TrueHD analyser (`sw-codec-truehd/src/analyze.rs`, `sw-verify/src/thd.rs`) reports 0 blocks / "bad Huffman code" on every
substream of Dolby's Atmos TrueHD: a syntax variant (likely extra restart-header fields) is unhandled. FFmpeg strict decode is
the verdict meanwhile.

### Other recorded findings
- Express: sequential by nature (a frame's tone choice and rate search feed each other); ~5-7x real time.
- AC-3: exponent strategy choices tried, all within 0.2 dB: fixed choice stays. ALS: block switching in (+0.1-0.3%), longer
  orders gained 0.02% (limit 64).
- DTS 96 kHz MA parity needed L48=480, phase 0.5 (the old gap was a 487-sample misalignment). PR QMF table now from reference
  values.
- Speed: Windows malloc lock hurt scaling (mimalloc + Arc for big tables); `rayon::spawn` + blocking recv deadlocks on one
  thread (use `rayon::join`); the old 1.2 s fixed cost was `peak_bit_rate_note` scanning audio outputs (removed).

---

## 8. Build, run, test environment (Windows)

### 8.1 Toolchain
- **Always** `export RUSTUP_TOOLCHAIN=stable-x86_64-pc-windows-gnu` in each Bash call (no persistent env; no MSVC linker
  here: the MSVC target picks up Git's `link`). `rust-toolchain.toml` says plain `stable`. Local clippy was 1.97, CI's newer
  (1.99): add `allow`s to `[workspace.lints]` rather than chase new lints (already allowed: needless_range_loop,
  too_many_arguments, type_complexity, field_reassign_with_default, chunks_exact_to_as_chunks; rust `unknown_lints=allow`).
- Node is portable: `export PATH="/e/Tools/node-v24.21.0-win-x64:$PATH"`. App: `cd app && npx tauri build --debug --no-bundle`
  (~50 s; debug exe ~350 MB, release ~9 MB). WebView2 profile for tests: `E:\Tools\wv2data`.
- Release flags: `lto = "fat"`, `codegen-units = 1`, `strip = "symbols"`; dev `opt-level = 2` (DSP is unusable at 0).
- `rustfmt.toml`: `max_width = 120`.

### 8.2 Test material (local only; never commit)
- `O:\MEDIA\Unsorted\CITY OF GOD REMUX\CoG-PCMsplitfiles`: 5.1 mono 24-bit/48k stems `CityOfGod_JPN_{L,R,C,LFE,Ls,Rs}.wav`
  (2h09m55s; loud at 10:00 and 14:46), plus Commentary L/R.
- `F:\SubwoofaTests\watchtest\in\done\T1_*.wav, T2_*.wav`: 8 s 5.1 excerpts of CoG (at 600 s and 900 s); T2's centre peaks -1.4
  dBTP; the workhorse for quick encodes, `verify --against`, Dolby comparisons. `F:\SubwoofaTests\dee\` (engine experiments:
  `out/`, `ref/`, `s71/` 7.1 set, schema.xsd), `F:\SubwoofaTests\dp` (Deep Purple stereo 96k: `m_lpcm.wav`, commercial TrueHD
  `m_th.thd`, MA `m_ma.dts`, AC-3), `F:\SubwoofaTests\DOLBY ATMOS RENDERER TEST FILES` (DAMF stubs `ObjectTest`, `DATF`;
  ADM BWFs `CRUSH ON ADM - ACTUALLY POSITIONED/NOT POSITIONED - WAV.wav`, 153 s each, from DaVinci Resolve), many older
  `.thd/.dtshd/.ac3` experiment outputs.
- `E:\FormatProgram\TEST FILES`: Dire Straits Sultans of Swing (DTS-HD MA 5.1 96k + mono PCM), Lady Gaga You and I (TrueHD
  96k + PCM), Oppenheimer (Atmos master PCM + DTS-HD MA 5.1), Shrek (DTS:X, marker 0x02000850 after the XLL audio), Tokyo 2020
  opening (AAC 22.2 + 24 mono WAV + .mka), Terrifier 3 (8ch PCM + mono split).
- `O:\MEDIA\Unsorted\forSubwoofa\DEEP_PURPLE_MADE_IN_JAPAN`: Blu-ray with LPCM + DTS-HD MA + TrueHD + AC-3 of one stereo 96k
  programme (commercial MA/TrueHD decode bit-identical to LPCM; ours too).
- `E:\FormatProgram\TEST FILES\OnlineTestFiles` (added 2026-10-16; public sample clips):
  - `DolbyAC4DeliveryKitv1.5\`: Dolby's AC-4 kit: 2.0, 5.1, 5.1.4 and IMS at 25 and 29.97 fps (frame rate and bit rate in
    the file names: the ground truth for the AC-4 check), plus the same clip as AAC and DD+.
  - `PhotoprismAC4\`: A-JOC, channel-based (44.1 kHz) and IMS (two presentations) AC-4. `PhotoprismWAV\`: odd WAVs (A-law,
    mu-law, GSM, u8, 9 ch, 3 kHz, 192 kHz, a layout change) for input-reader robustness.
  - `KodiAudioTestClips\`: `DTS-UHD DTSX P2 Sample.mp4` (DTS-UHD in MP4, 10 ch, 6 241 frames, 133.141 s; FFmpeg cannot
    decode it); DTS:X discs `DTSX Emulator.mkv`, `DTSX Gravity.mkv`, `DTSX_Demo_2016.m2ts`; `Dolby Amaze.ATMOS.m2ts` (TrueHD
    Atmos, AC-3, and Blu-ray DD+ Atmos 7.1 = AC-3 bsid 6 + E-AC-3 dependent frames); `EAC3-JOC ATMOS.mkv`; `DTS-HRA 7.1
    Orchestra.mkv` (96 kHz HRA 7.1); `DTS-HD MA 7.1 'Dredd'` and THX Deep Note (MA); spoken channel-test FLACs (5.0-7.1 at
    44.1-192 kHz) and `TrueHDChannelTest.mkv` for listening checks of channel order.
  - Elementary streams taken out of them (`ffmpeg -i X -map 0:a:N -c copy -f dts|truehd|eac3|data`) are in
    `F:\SubwoofaTests\online` (`uhd.raw`, `amaze_atmos.thd`, `amaze_joc71.eac3`, `dtsx_*.dtshd` ...). `amaze_atmos.thd`
    reproduces the TrueHD-analyser gap with Atmos substreams (open item 4).
- Test outputs always go to `F:\SubwoofaTests` (C: is nearly full).

### 8.3 The test suite
- `cargo test --workspace --exclude subwoofa-app --no-fail-fast` (**use `--no-fail-fast`**: without it one failure skips the
  remaining binaries, which once hid a flaky test). ~330 tests, ~2 min after the first build.
- Tests needing FFmpeg skip themselves without it; DEE tests (`sw-backend-dee/tests/dee.rs`, `tasks.rs`) skip without the
  engine. The CI runner probably has neither, so those tests only really run on the user's machine (check a CI log before
  relying on them there); DCP tests also use asdcplib if `SW_ASDCP_DIR` is set.
- **Golden checksums**: `crates/sw-queue/tests/golden.rs` holds an md5 for ~60 encodes covering every format (incl. 96k 6.1/7.1
  HRA, DTS CD, stereo set, 7.1.4, 22.2, TrueHD fast/balanced/best/96k...). Any intentional output change shows as "output
  changed: name: expected X, made Y". Update the three lines deliberately (print all with `SW_UPDATE_GOLDEN=1`; keep outputs with
  `SW_GOLDEN_DIR`). Refactors must leave it unchanged: record baselines from the OLD code first, keep an old binary
  (`/tmp/old_sub.exe`) to compare verify JSON.
- Other key tests: `sw-queue/tests/compare.rs` (compare, preview, normalise, delay, FLAC/AC-3 round trips), `queue.rs`,
  `sw-codec-truehd/tests/roundtrip.rs` (25 tests incl. substream-0-alone must equal the downmix formula), `sw-codec-ac3`
  edit tests, `sw-backend-dee` profile + real-engine tests, `sw-cli` job-file tests.
- A test that spawns FFmpeg or writes temp files must use unique temp paths (include every parameter in the name: a race in
  the DTS LBR LFE tests came from two tests sharing one file). Never compare serialised `HashMap`s as strings (order random).

### 8.4 Driving the Tauri app for real tests (no computer-use: it is not an installed app)
Launch the debug app with the remote-debugging port and drive it by the DevTools protocol (Node 24 has `WebSocket`):
```powershell
$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222"
$env:WEBVIEW2_USER_DATA_FOLDER="E:\Tools\wv2data"
Start-Process "E:\FormatProgram\subwoofa\target\debug\subwoofa-app.exe"   # wait ~8 s
```
`cdp.mjs <shot.png|-> "<async JS expression>"` (write expressions to a file and pass `"$(cat file.js)"`; Bash `$` and
backslashes eat inline expressions):
```js
import fs from "node:fs";
const [shot, expr] = process.argv.slice(2);
const list = await (await fetch("http://127.0.0.1:9222/json")).json();
const ws = new WebSocket(list.find((p) => p.type === "page").webSocketDebuggerUrl);
let id = 0; const pending = new Map();
ws.onmessage = (m) => { const d = JSON.parse(m.data); if (d.id && pending.has(d.id)) { pending.get(d.id)(d); pending.delete(d.id); } };
await new Promise((r) => (ws.onopen = r));
const send = (method, params = {}) => new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
if (expr) { const r = await send("Runtime.evaluate", { expression: expr, awaitPromise: true, returnByValue: true });
  console.log(JSON.stringify(r.result.result.value ?? r.result.exceptionDetails ?? r.result.result, null, 1)); }
if (shot !== "-") { const r = await send("Page.captureScreenshot", { format: "png" }); fs.writeFileSync(shot, Buffer.from(r.result.data, "base64")); }
ws.close();
```
Useful page-level handles: `inputs` (channel -> path; changing dest/stream/layout **clears it**, set files after choosing the
format), `outDir`, `jobSpec(inputs, name)`, `invoke(cmd, args)`, `startPreview()`, `watchTick()`, `setMode("tools")`,
`atm.dee`, `cat`. Emulate the minimum window with `Emulation.setDeviceMetricsOverride` 1200x780. A text-clipping scan
(`scrollWidth > clientWidth` with overflow hidden/ellipsis over every mode, tab, settings page and several formats) found
zero clipped texts at the end of the session: rerun it after UI edits. Always run `node --check` on edited JS.

### 8.5 Installer and release builds
- `app/src-tauri/tauri.conf.json`: NSIS only, `installMode: currentUser`, Start menu folder "Subwoofa", WebView2 bootstrapper
  embedded (does not speed the runtime install), `resources`: `fetch-ffmpeg.ps1`, `path.ps1`, `installer/stage/subwoofa.exe`
  (the CLI, staged by hand locally and by a workflow step in CI; git-ignored), custom template `installer/installer.nsi`
  (a copy of Tauri's NSIS template with ONE added line, `!insertmacro SUBWOOFA_OPTIONS_PAGE`, before the install-files page;
  re-diff against Tauri's upstream template when upgrading the CLI), `installer/hooks.nsh` (Extras page with nsDialogs: Add to
  PATH [default on], Download FFmpeg [default on unless found]; post-install runs `path.ps1 -Action add` and
  `fetch-ffmpeg.ps1`; pre-uninstall removes the PATH entry unless updating, and removes the downloaded FFmpeg only if "Delete the
  application data" is ticked), artwork `header.bmp`/`sidebar.bmp` drawn by `make-art.ps1`.
- `path.ps1` edits the user PATH via the registry (REG_EXPAND_SZ, long PATHs fine: the user's is 1215 chars, over NSIS's
  1024-char strings), broadcasts WM_SETTINGCHANGE.
- Silent installs (`/S`) skip pages and do nothing extra. Silent test: `Subwoofa-setup.exe /S /D=E:\Tools\swrel` then
  `uninstall.exe /S` (the page callback must `Abort` when `${Silent}`/passive; it once hung).
- **Local release build** (~6-8 min): `cargo build --release -p sw-cli`, copy `target/release/subwoofa.exe` to
  `app/src-tauri/installer/stage/`, `cd app && npx tauri build --bundles nsis`. Output: installer 5.2 MB, app 9.2 MB, CLI 4.4 MB.
  The user's last requests put results in `E:\FormatProgram\subwoofa\dist\` (git-ignored): `Subwoofa-setup.exe`,
  `Subwoofa-portable.zip` (`Subwoofa.exe` app + `subwoofa-cli.exe` CLI; **never name both `subwoofa.exe`**: Windows is
  case-insensitive and one overwrites the other), `SHA256SUMS.txt`.
- **CI**: `.github/workflows/ci.yml` (windows-latest, GNU toolchain, fmt check, clippy `-D warnings` excluding the app, build,
  test). `release.yml`: tag `b<version>-YYYY-MM-DD-HH-MM-SS` -> tests, CLI + app build, installer, portable zip, SHA256SUMS,
  pre-release. Manual "Run workflow" = dry run artifact. `SUBWOOFA_BUILD_STAMP` is set there. **Never create the tag yourself.**
- Releases are unsigned (SmartScreen warns). Code signing = open, with the user (free options like SignPath need their account).

---

## 9. The check-before-push routine (do all of it)

```
taskkill //F //IM subwoofa-app.exe          # if the app is running
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings          # CI excludes subwoofa-app; locally include it
cargo test --workspace --exclude subwoofa-app --no-fail-fast > /tmp/t.log 2>&1
grep -cE " FAILED|panicked" /tmp/t.log      # must be 0
```
Then for UI changes: `node --check` every `app/ui/*.js`, rebuild the debug app, drive it (section 8.4). Then commit, push, and a few
minutes later `gh run list --limit 3`. Never chain the push after a command that can fail silently: `grep -c` returns 0
matches with exit status 1, and `;` chains continue after failures (one push went out with a failing test this way).

---

## 10. Documentation map (keep these in step with the code)

`README.md` (public face; screenshot `docs/images/screenshot.png` = current UI), `CHANGELOG.md` (Unreleased section = everything since
the 2026-10-11 pre-release; add a bullet per user-visible change), `docs/`: `ARCHITECTURE.md` (map + how to add a format), `FORMATS.md`
(state table), `USAGE.md` (CLI,
job files, watch, dialnorm, normalise), `ENGINES.md` (FFmpeg, DTS MAS, DEE, "Beyond Atmos" section), `ATMOS.md`, `SPEED.md`
(measured 2026-10-12..14: TrueHD 2.4% smaller than FFmpeg's; the TrueHD numbers there predate the 3% improvement), `GETTING-STARTED.md`,
`TROUBLESHOOTING.md`, `LEGAL.md`, `ROADMAP.md` (older, partly stale; TODO.md is the live list), `TODO.md` (the live list + the
code standard), `REFERENCE-ENCODES.md`, `docs/agents/*` (issue tracker = GitHub via `gh`, triage labels, domain-docs layout;
there is no `GLOSSARY.md` or `docs/adr/` yet). `CONTRIBUTING.md`, `SECURITY.md`.

---

## 11. Code standard and conventions

New code (from `docs/TODO.md`): (1) every file opens with a module comment: what it is for, which spec (section/table names),
order of the pieces; (2) names are short words (`step`, `window`; not `qs`, not sentences; test names 2-3 words); (3) functions
do one thing, ideally < 40 lines, lines < ~110 chars (rustfmt enforces 120); (4) constants and tables are named and say where they
come from; (5) comments explain why; (6) each piece has a test that fails when it is wrong, and round trips use an independent
decoder; (7) no dead code or debug leftovers (clippy `-D warnings` with `dead_code` catches unused struct fields: it failed on a
leftover `coarse` field once).
Refactor safety net: golden checksums + old-vs-new binary comparisons. Three readability rounds are done (splits along format
stages, long functions into named steps). Left: six functions of 90-106 lines in DTS/TrueHD (`next_frames` in
`dca/hra/hra96/encoder.rs`, `analyze_substream`, `report_checks` in `sw-verify/src/thd.rs`, Express `encode_to_file`, `lpc_band`,
...). Find long functions with a small Node script (a brace-matching line counter; the one used is not saved).
The UI standard: every explanation lives in an (i) (`INFO` list in `app/ui/tabs.js`; selector `@...` names the label itself), no
visible paragraphs, nothing clipped, `hidden` must really hide (`.srow[hidden]`, `.chk[hidden]`, `.f[hidden]`, `.op-row[hidden]`
have explicit rules).

---

## 12. Traps and lessons (each cost time)

- Shell: Git Bash. `cat <<'EOF'` heredocs with an odd apostrophe can fail to parse **before running anything**; `$` and
  backslashes get eaten in `node -e '...'`. Use the Write tool for scripts, run `node file.js`. `perl -0pi` with `$1` mangled
  `job.js` and `release.yml` (text spliced into line 1). `sed -i` with `\s` loses the backslash. PowerShell `Remove-Item`/
  `Compress-Archive -Path dist\portable\*` can trip the harness's protected-path guard (use `-LiteralPath`; do copies in Bash).
- Background builds: `cmd &` inside a tool call dies with the shell; use `run_in_background: true` on the real command.
- `grep -c` exit codes and `;` vs `&&` chains (section 9). `cargo test` stops at the first failing binary without `--no-fail-fast`.
- `StemSet.read_planar` returns files' channels already mapped to layout slots; mono files feed one channel, stereo files
  supply their right channel to the next slot (so a stereo output WAV needs both `L` and `R` entries pointing at it).
- `Object.values(g.inputs)` order from `scan_folder` is random per call (Rust HashMap): sort before fingerprinting.
- Changing Destination/Stream/Layout in the page clears `inputs`; set files afterwards in any scripted test.
- `outDir`, `inputs` are page globals; `$("ext")`, `$("outname")` drive the auto name; `update()` recomputes enablement.
- `previewBtn`/Encode enablement uses `blockedByEngine()` for normalise/delay vs non-native formats and the dolby setting.
- The DEE and FFmpeg path settings reach Rust through `set_tool_paths` / `set_dee_path` (page `store` = localStorage).
- E-AC-3 ids: stream `eac3` (file `.eac3`/`.ec3`); AC-3 input stream type has no `stream_type` element in DEE XML, E-AC-3 does.
- TrueHD destination: `generic` works for `truehd` (bluray-primary rejected it with `StreamNotInDestination` at 48k).
- DTS-HD MA/`.dtshd` outputs are git-ignored (`*.dtshd`, `*.cpt`, `*.log`).
- Windows Sandbox needs the user to run the installer; WebView2 install there is slow and unfixable from our side.
- Use `SendUserFile` for files; the limit is 30 MB (debug installers 46-66 MB do not fit, release 5.2 MB does).
- The harness design hook flags tiny text/cramped padding in `index.html` as "attribution unknown": the dense 11-12 px control-surface
  look is deliberate (stated in `style.css`); no ignore rules were added. Raising sizes would be a whole-UI design pass, ask first.

---

## 13. Open work, in the order I would do it

**Needs the user (do not start without them):** Windows Sandbox test of the new installer (Extras page layout, PATH, FFmpeg
step, uninstall); a listening test of Preview; the decision on the Opus review pass; code signing together; the next release tag
(the published pre-release predates ~250 commits of features); any AI-desloping of UI strings (deferred by the user).

**Unblocked, valuable:**
1. **Opus (or higher-effort) review pass**, findings only (no edits), over: byte-exact encoders (`sw-codec-dts`, `sw-codec-truehd`),
   `sw-verify`, `sw-loudness`/`source.rs`/`preview.rs`, Tauri file handling and the NSIS/PowerShell installer scripts. Triage findings
   against golden + FFmpeg before changing anything.
2. **E-AC-3 enhanced coupling** (encoder side). Needs ETSI TS 102 366 Annex E text (fetch the public PDF, do not rely on memory);
   verify with Dolby's decoder (`subwoofa dolby decode`) since FFmpeg's decoder lacks it. Only matters at low bit rates; no perceptual
   metric exists here, so judge by the Dolby decode vs source.
3. **TrueHD predictor variety** (ARMA/IIR search, FIR order mix, Huffman offsets) toward the last 0.7-1.4% against Dolby; use the
   `--engine dolby` reference and `stream-info`. Keep the golden file updated deliberately (only TrueHD lines).
4. **Our TrueHD analyser vs Dolby's Atmos substreams** (find the unhandled restart-header variant).
5. **Roadmap #5**: video / existing-stream inputs (warn on lossy to lossy), frame-rate conversion (23.976/24/25, with/without pitch:
   without needs a time-stretch, probably FFmpeg `atempo`). **#6** DRC profile and DTS/TrueHD header edits without re-encode (needs DTS
   reference files). **#7** downmix design (BS.775 presets, loudness-matched stereo, picture of speaker feeds). **#8** macOS/Linux builds,
   winget/scoop manifests (prepare only once a signed/hosted release exists), a "first disc in ten minutes" tutorial.
6. **Automation leftovers**: headless encode server, DAW stem-naming conventions in auto-import. **Preview leftovers**: multichannel monitor,
   `subwoofa preview` CLI writing the three WAVs. **Dolby options** not exposed (timecode base, embedded timecodes, log format).
7. **Readability tail** (six 90-106-line functions; a few long names). **Speed leftovers**: Express (~5-7x), single-core AC-3/TrueHD 3-4x slower
   than FFmpeg, FLAC 1.0 s floor.
8. **Docs hygiene**: refresh `ROADMAP.md` (it still lists done items as upcoming), update SPEED.md's TrueHD
   numbers, README feature table (mention Dolby engine extras, preview, watch), `docs/ENGINES.md` already current.
9. **Add a `GLOSSARY.md` and `docs/adr/`** (the agent-skills config expects them) if domain terms (substream, restart interval, presentation,
   dialnorm, XLL...) are to be recorded; none exists.

**Blocked on outside material:** HRA 6.1 at 48 kHz, DVD-Audio AOB structure (a real disc), Atmos Theatrical IAB (a cinema track file), DTS:X
encoding (DTS's licensed encoder), AAF import (AAF files), the 13 Bitstream-tab reference encodes (mostly done already; see lab doc),
real-hardware tests (discs, players, a cinema server, a DVD-Audio player).

**Parked:** AC-4, MPEG-H, Auro-3D, Datasat.

---

## 14. Memory notes

Claude's persistent memory for this project is at `C:\Users\Callum\.claude\projects\E--FormatProgram\memory\` (index `MEMORY.md`): project,
dev environment, Atmos/DEE/DTS:X, audio files, original-encoder lab, release prep, readable-code feedback, speed round, readability round,
roadmap progress. They overlap with this file; if you update one, update the other. The harness may re-announce deferred tools; the Skill
list includes `mattpocock-skills:*`, `impeccable:*` (design critique commands), `anthropic-skills:*` (unslop-ui etc.). The user's UI-design
skill hook (`impeccable`) runs after UI edits and flags findings; triage per section 12.

---

## Agent skills

### Issue tracker

Issues live in GitHub Issues for callumcomplains/subwoofa (via the `gh` CLI). See `docs/agents/issue-tracker.md`.

### Triage labels

Default vocabulary: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: one `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.
