// Turning the page into a job: dialnorm, the output name, speed, the job specification, and the queue list.

/** Dialnorm as a positive attenuation 1..=31; off always means 31 (-31 dBFS, no attenuation). */
function dialnormValue() {
  if (!$("dialnormOn").checked) return 31;
  const v = Math.abs(Math.round(Number($("dialnorm").value)));
  return Number.isFinite(v) && v >= 1 ? Math.min(v, 31) : 31;
}
/** Measures the input files and sets the dialnorm from the integrated loudness. */
async function measureLoudness() {
  const out = $("loudReadout");
  const files = {};
  for (const ch of inLayout().channels) if (inputs[ch]) files[ch] = inputs[ch];
  if (!Object.keys(files).length) { out.hidden = false; out.className = "readout bad"; out.textContent = "Choose the input files first."; return; }
  const entire = $("entire").checked;
  const btn = $("dialnormMeasure");
  btn.disabled = true;
  out.hidden = false; out.className = "readout"; out.textContent = "Measuring...";
  try {
    // Dolby's meter finds the dialogue first, which is what a dialnorm should describe; ours measures the whole programme
    const dolbyMeter = dolbyFound() && $("dnDolby").checked;
    const l = dolbyMeter
      ? await measureWithDolby(files, entire)
      : await invoke("measure_loudness", { layout: inLayout().id, inputs: files, channelOrder, start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value) });
    if (l.integrated == null) { out.className = "readout bad"; out.textContent = "The input is silent."; return; }
    const tp = dolbyMeter ? l.peak : l.channels.reduce((m, c) => (c.true_peak != null && c.true_peak > m ? c.true_peak : m), -Infinity);
    const dn = Math.min(31, Math.max(1, Math.round(-l.integrated)));
    $("dialnormOn").checked = true; $("dialnorm").disabled = false; $("dialnorm").value = -dn;
    syncDialnorm();
    const over = tp > -1;
    out.className = over ? "readout flag" : "readout";
    out.textContent = `${l.integrated.toFixed(1)} LUFS${dolbyMeter ? " on the " + l.how : ""}, range ${l.range == null ? "-" : l.range.toFixed(1)} LU, true peak ${tp.toFixed(1)} dBTP${over ? " (over -1 dBTP)" : ""}`;
  } catch (e) { out.className = "readout bad"; out.textContent = String(e); }
  finally { btn.disabled = false; }
}

/** The Normalise row: the target field follows the checkbox, and a format that goes through FFmpeg cannot use it. */
function syncNormalise() {
  const on = $("normOn").checked;
  $("normTarget").disabled = !on;
  const unsupported = on && curStream() && !curStream().native;
  $("normNote").textContent = unsupported ? "this format needs the native encoder" : on ? "peaks stay under -1 dBTP" : "";
  $("normNote").className = "readout" + (unsupported ? " bad" : "");
}

function syncDialnorm() {
  const on = $("dialnormOn").checked;
  const raw = Math.abs(Math.round(Number($("dialnorm").value)));
  const bad = on && !(raw >= 1 && raw <= 31);
  $("dialnorm").classList.toggle("bad", bad);
  $("dialnormNote").textContent = !on ? "off (-31)" : bad ? "enter -1 to -31" : `-${dialnormValue()} dBFS`;
  if ($("variants").dataset.value) update();   // skip during start-up, before a layout exists
}

const DEST_TAG = { "bluray-primary": "BD", "bluray-secondary": "BD Secondary", dvd: "DVD", "digital-delivery": "Digital", "dts-music-disc": "DTS Disc" };
const STREAM_TAG = { "dtshd-ma": "DTSHDMA", "dtshd-hra": "DTSHDHRA", "dts-es": "DTSES", "dts-96-24": "DTS9624", dts: "DTS", "dts-express": "DTSExpress", ac3: "AC3", eac3: "EAC3", truehd: "TrueHD", mlp: "MLP", wav: "WAV", aiff: "AIFF", caf: "CAF", flac: "FLAC", oggflac: "OggFLAC", alac: "ALAC", tta: "TTA", dsf: "DSD", dff: "DSD", "dcp-audio": "DCP", wavpack: "WavPack", ape: "APE", als: "ALS" };
let nameAuto = true;   // false once the user types their own name

/** 5.1, 7.1, 5.1.4 ... from a layout's channel names. */
function layoutLabel(ch) {
  const lfe = ch.filter((c) => /^LFE/i.test(c)).length;
  const hi = ch.filter((c) => /^(Lh|Rh|Ch|Lhs|Rhs|Lhr|Rhr|Chr|Oh|Lts|Rts|Ts|Vhc)/.test(c)).length;
  const bed = ch.length - lfe - hi;
  return `${bed}.${lfe}${hi ? "." + hi : ""}`;
}

/** "SUBWOOFA - BD DTSHDMA 7.1 96kHz 24FPS": the first input's name (or SUBWOOFA) plus what the stream is. */
function autoName() {
  const first = Object.values(inputs).find(Boolean);
  const stem = first ? base(first).replace(/\.[^.]*$/, "").replace(/[ ._-](L|R|C|LFE|Ls|Rs|Lss|Rss|Lsr|Rsr|Cs|FL|FR|FC|SL|SR|BL|BR)$/i, "") : "";
  const parts = [DEST_TAG[$("dest").value], STREAM_TAG[$("stream").value] || $("stream").value, layoutLabel(curLayout().channels)];
  const r = curRate();
  if (r > 0) parts.push(`${r / 1000}kHz`);
  const st = curStream();
  if (st && !st.lossless && $("bitrate").value) parts.push(`${$("bitrate").value}kbps`);
  if (!$("bsTcWrap").hidden) parts.push(`${($("bsFps").value || "24").replace(" drop", "d")}FPS`);
  return `${stem || "SUBWOOFA"} - ${parts.filter(Boolean).join(" ")}`;
}

function syncName() {
  if (!nameAuto || !$("variants").dataset.value || !curStream()) return;
  $("outname").value = autoName();
}

function update() {
  syncName();
  const need = inLayout().channels;
  const dnOk = !$("dialnorm").classList.contains("bad");
  const ready = need.every((c) => inputs[c]) && outDir && $("outname").value.trim() && !rateMismatch().length && bitrateOk() && dnOk && (cat.has_ffmpeg || curStream().native) && curRate() > 0;
  const available = curStream().available && (!srcLayoutId || curStream().native);
  $("go").disabled = !ready || !available || ($("normOn").checked && !curStream().native);
  $("previewBtn").disabled = !pv.busy && !previewReady();
  if (!available && srcLayoutId) log("Mixing 22.2 down needs a format with a native Subwoofa encoder.");
  else if (!available) log(`${curStream().name} has no encoder yet. It is on the roadmap; for now this format is listed so you can see what is coming.`);
  syncChips();
  syncSpeed();
  syncNormalise();
}

/** The speed buttons and the note under them: what the chosen setting costs for the chosen format. */
function syncSpeed() {
  const cost = curStream()?.speed_cost;
  for (const b of $("speedSeg").children) {
    b.setAttribute("aria-pressed", String(b.dataset.speed === speed));
    b.disabled = !cost;
  }
  const note = $("speedNote");
  const shown = cost && speed !== "best";
  note.hidden = !shown;
  if (shown) {
    note.textContent = `${speed === "fast" ? "Fast" : "Balanced"}: this file comes out ${cost[speed]} than Best would make it.`;
    note.classList.toggle("costly", speed === "fast");
  }
  $("speedBox").title = cost ? "" : "This format has one speed.";
}

function jobSpec(inputsFor, name) {
  const entire = $("entire").checked;
  return {
    dest: $("dest").value, stream: $("stream").value, layout: curLayoutId(),
    sample_rate: curRate(),
    bitrate: coreOff() ? 0 : $("bitrate").disabled || !$("bitrate").value ? null : Number($("bitrate").value),
    dialnorm: dialnormValue(), unlocked, source_layout: srcLayoutId || null, engine: useDolbyEncoder() ? "dolby" : srcLayoutId || (curStream().native && (preferNative || !curStream().ffmpeg)) ? "native" : "ffmpeg", downmix: thdDownmix, channel_order: channelOrder,
    pcm_format: ["wav", "aiff", "caf", "flac", "oggflac", "alac", "tta", "wavpack", "ape", "als", "dsf", "dff"].includes($("stream").value) ? $("pcmFormat").value : "", es_phase_shift: $("esShift").checked, rear_minus_3db: $("rear3").checked, core_x96: $("x96Core").checked,
    start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
    inputs: inputsFor, output: outDir + "\\" + name + $("ext").textContent,
    bitstream: bitstreamSpec(),
    normalize_lufs: $("normOn").checked && Number.isFinite(Number($("normTarget").value)) ? Number($("normTarget").value) : null,
    speed, threads,
  };
}

async function encode() {
  try {
    const id = await invoke("queue_add", { verify: autoVerify, spec: jobSpec(inputs, $("outname").value.trim()) });
    log(`Queued as job ${id}.`, "ok");
  } catch (e) { log(String(e), "bad"); }
}

/** Queue one job per complete set of channel files in a folder, with the current format settings. */
async function batchFromFolder() {
  if (!outDir) { log("Choose an output folder first.", "bad"); return; }
  const dir = await dialog.open({ directory: true, multiple: false });
  if (!dir) return;
  try {
    const groups = await invoke("scan_folder", { dir, layout: inLayout().id });
    if (!groups.length) { log(`No complete sets of ${inLayout().channels.length} channel files (named like Name_L.wav, Name_R.wav ...) found in that folder.`, "bad"); return; }
    let n = 0;
    for (const g of groups) {
      const safe = g.name.replace(/[\\/:*?"<>|]/g, "_");
      await invoke("queue_add", { verify: autoVerify, spec: jobSpec(g.inputs, safe) });
      n++;
    }
    log(`Queued ${n} job${n === 1 ? "" : "s"} from ${dir}: ${groups.map((g) => g.name).join(", ")}.`, "ok");
  } catch (e) { log(String(e), "bad"); }
}

/* ---------- queue ---------- */

let autoVerify = true;
let preferNative = false;
let thdDownmix = true;
let channelOrder = "auto";
const fmtClock = (s) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, "0")}`;

function renderQueue(jobs) {
  $("queue").hidden = jobs.length === 0;
  const active = jobs.filter((j) => !["passed", "completed", "failed", "canceled"].includes(j.state)).length;
  if (active || jobs.length === 0) {
    $("qCount").textContent = `${jobs.length} job${jobs.length === 1 ? "" : "s"}${active ? `, ${active} active` : ""}`;
  } else {
    // the batch is over: say how it went
    const count = (...states) => jobs.filter((j) => states.includes(j.state)).length;
    const parts = [];
    if (count("passed")) parts.push(`${count("passed")} passed`);
    if (count("completed")) parts.push(`${count("completed")} done`);
    if (count("failed")) parts.push(`${count("failed")} FAILED`);
    if (count("canceled")) parts.push(`${count("canceled")} canceled`);
    const total = jobs.reduce((t, j) => t + (j.elapsed_secs || 0), 0);
    $("qCount").textContent = `${jobs.length} job${jobs.length === 1 ? "" : "s"}: ${parts.join(", ")} - ${fmtClock(total)} in all`;
    $("qCount").classList.toggle("bad", count("failed") > 0);
  }
  const ul = $("qList");
  ul.innerHTML = "";
  for (const j of jobs) {
    const li = document.createElement("li");
    const done = j.state === "passed" || j.state === "completed";
    const fail = j.state === "failed" || j.state === "canceled";
    const live = j.state === "running" || j.state === "verifying";
    const label = { pending: "Waiting", running: "Encoding", verifying: "Verifying", passed: "Passed", completed: "Done", failed: "Failed", canceled: "Canceled" }[j.state];
    li.innerHTML = `<span class="qstate ${j.state}">${label}</span>
      <span class="qname" title="${j.output.replace(/"/g, "&quot;")}">${j.name}${j.message ? `<small class="${j.state === "failed" ? "bad" : ""}" title="${j.message.replace(/"/g, "&quot;")}">${j.message.split(String.fromCharCode(10))[0]}</small>` : ""}</span>
      <span class="bar-track"><span class="bar-fill ${done ? "done" : fail ? "fail" : ""}" style="display:block;width:${Math.round(j.progress * 100)}%"></span></span>
      <span class="qtime">${live || done || fail ? fmtClock(j.elapsed_secs) : ""}${j.state === "running" && j.progress > 0.03 ? `<small> ~${fmtClock(j.elapsed_secs * (1 - j.progress) / j.progress)} left</small>` : ""}</span>
      <span class="qbtns"></span>`;
    const btns = li.querySelector(".qbtns");
    const add = (txt, title, fn) => { const b = document.createElement("button"); b.textContent = txt; b.title = title; b.className = "plain"; b.onclick = fn; btns.append(b); };
    if (j.state === "pending" || live) add("Cancel", "Cancel this job", () => invoke("queue_cancel", { id: j.id }));
    if (!live) add("Log", "Show this job's log", async () => { try { log(await invoke("read_log", { path: j.log_path })); } catch (e) { log(String(e), "bad"); } });
    if (!live) add("✕", "Remove from the list", () => invoke("queue_remove", { id: j.id }));
    ul.append(li);
  }
}
