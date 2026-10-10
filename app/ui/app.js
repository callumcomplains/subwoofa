const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const $ = (id) => document.getElementById(id);

let cat;                 // catalog from the Rust side
let inputs = {};         // channel -> path
let srcLayoutId = "";    // "" = inputs match the output layout; "22.2" = NHK 22.2 inputs mixed down
let meta = {};           // channel -> probe result
let outDir = "";
let opts = null;         // last options() answer
let unlocked = false;

const COMMON_RATES = [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000];
const DEFAULT_BITRATE = { ac3: 640, eac3: 1024, dts: 1509, "dts-express": 256 };

// Plain-language names for each layout variant (the id is stable; this is only for display).
const VARIANT = {
  "7.1-side-rear": "Side + rear surrounds", "7.1-ls-rs-lsr-rsr": "Surrounds + rear surrounds",
  "7.1-height": "Front heights", "7.1-height-side": "Side heights", "7.1-cs-ch": "Center surround + center height",
  "7.1-cs-oh": "Center surround + overhead", "7.1-wide": "Wide fronts", "7.0-side-rear": "Side + rear surrounds",
  "6.1-discrete": "Discrete (ES)", "6.0-discrete": "Discrete", "6.1-matrix": "ES matrix", "6.0-matrix": "ES matrix",
  "6.1-matrix-premixed": "ES matrix, pre-mixed", "6.0-matrix-premixed": "ES matrix, pre-mixed",
  "5.1": "Standard", "5.0": "Standard", "4.1-lcrs": "L C R + surround", "4.1-quad": "Quad",
  "4.0-quad": "Quad", "4.0-lcrs": "L C R + surround", "3.1": "L C R", "3.1-lrs": "L R + surround",
  "3.0": "L C R", "3.0-lrs": "L R + surround", "2.1": "L R + sub", "2.0": "Stereo", "2.0-ltrt": "Lt/Rt matrix",
  "1.1": "Mono + LFE (DVD only)", "1.0": "Mono", "22.2": "NHK, lossless only",
  "9.1.6": "Immersive, lossless only", "7.1.4": "Immersive, lossless only", "7.1.2": "Immersive, lossless only",
  "5.1.4": "Immersive, lossless only", "5.1.2": "Immersive, lossless only",
};

// Angle in degrees clockwise from front-centre, plus ring (0 ear level, 1 height, 2 LFE, 3 overhead).
const POS = {
  L: [-30, 0], R: [30, 0], C: [0, 0], LFE: [0, 2], Ls: [-110, 0], Rs: [110, 0], Cs: [180, 0],
  Lss: [-90, 0], Rss: [90, 0], Lsr: [-140, 0], Rsr: [140, 0], Lw: [-60, 0], Rw: [60, 0],
  Lh: [-45, 1], Rh: [45, 1], Lhs: [-90, 1], Rhs: [90, 1], Ch: [0, 1], Oh: [0, 3], Lt: [-30, 0], Rt: [30, 0],
  // NHK 22.2: second LFE, top layer, bottom layer (ring 4)
  LFE2: [30, 2], Tfc: [0, 1], Tfl: [-45, 1], Tfr: [45, 1], Tsl: [-90, 1], Tsr: [90, 1], Tc: [0, 3], Tbl: [-135, 1], Tbr: [135, 1], Tbc: [180, 1],
  Bfc: [0, 4], Bfl: [-30, 4], Bfr: [30, 4],
};

const family = (l) => (l.id === "22.2" || /^\d+\.\d\.\d$/.test(l.id) ? l.id : `${l.channels.filter((c) => c !== "LFE").length}.${l.channels.includes("LFE") ? 1 : 0}`);
const layoutById = (id) => cat.layouts.find((l) => l.id === id);
const curLayoutId = () => $("variants").dataset.value;
const curLayout = () => layoutById(curLayoutId());
const inLayout = () => (srcLayoutId ? layoutById(srcLayoutId) : curLayout());
const curDest = () => cat.destinations.find((d) => d.id === $("dest").value);
const curStream = () => cat.all_streams.find((s) => s.id === $("stream").value);
const curRate = () => ($("rate").value === "custom" ? Number($("ratecustom").value) : Number($("rate").value));
const base = (p) => p.split(/[\\/]/).pop();
const fmtRate = (r) => `${r / 1000} kHz`;
const fmtTime = (s) => {
  if (s == null) return "";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${sec.toFixed(3).padStart(6, "0")}`;
};
const store = {
  get(k) { try { return localStorage.getItem(k); } catch { return null; } },
  set(k, v) { try { localStorage.setItem(k, v); } catch { /* storage may be unavailable */ } },
};

function fill(sel, items, want) {
  sel.innerHTML = "";
  for (const [v, t] of items) sel.add(new Option(t, v));
  if (items.some(([v]) => String(v) === String(want))) sel.value = want;
}

function log(msg, cls = "") {
  const el = $("log");
  el.className = cls;
  el.textContent = msg;
  el.scrollTop = el.scrollHeight;
}

/* ---------- channel layout picker ---------- */

function renderLayoutPicker(validIds, chosen) {
  const valid = cat.layouts.filter((l) => validIds.includes(l.id));
  const fams = [...new Set(cat.layouts.map(family))];       // table order is richest first
  const have = new Set(valid.map(family));
  const sel = valid.find((l) => l.id === chosen) || valid.find((l) => l.id === "5.1") || valid[0];
  $("variants").dataset.value = sel ? sel.id : "";
  const selFam = sel ? family(sel) : "";

  const fe = $("families");
  fe.innerHTML = "";
  for (const f of fams) {
    const b = document.createElement("button");
    b.textContent = f;
    b.disabled = !have.has(f);
    b.setAttribute("role", "tab");
    b.setAttribute("aria-selected", String(f === selFam));
    b.onclick = () => pickLayout(valid.find((l) => family(l) === f).id);
    fe.append(b);
  }
  const ve = $("variants");
  ve.innerHTML = "";
  for (const l of valid.filter((l) => family(l) === selFam)) {
    const b = document.createElement("button");
    b.setAttribute("role", "option");
    b.setAttribute("aria-selected", String(l.id === sel.id));
    // Pre-mixed ES matrix layouts carry one fewer discrete channel than their name suggests.
    const name = l.id.includes("premixed") ? `${l.id.slice(0, 3)} ES matrix, centre surround pre-mixed` : `${selFam} ${VARIANT[l.id] || l.id}`;
    b.innerHTML = `<span class="vn">${name}</span><span class="vc">${l.channels.join(" ")}</span>`;
    b.onclick = () => pickLayout(l.id);
    ve.append(b);
  }
}

function pickLayout(id) {
  if (!srcLayoutId) { inputs = {}; meta = {}; }
  cascade("layout", id);
}

function drawSpeakers() {
  const l = curLayout();
  let h = `<circle class="ring" r="44"/><circle class="ring" r="82"/><path class="you" d="M0,-8 L6,6 L0,2.5 L-6,6 Z"/>`;
  for (const ch of l.channels) {
    const [deg, ring] = POS[ch] || [0, 0];
    let x, y;
    if (ring === 2) { x = deg ? 22 : 0; y = -21; }            // LFE (and LFE2 beside it): inside the inner ring, clear of the speakers
    else if (ring === 4) { const a = (deg * Math.PI) / 180; x = 62 * Math.sin(a); y = -62 * Math.cos(a); }   // bottom layer: small dots between the two rings, in front
    else if (ring === 3) { x = 0; y = 0; }                   // overhead: above the listener
    else { const r = ring === 1 ? 44 : 82, a = (deg * Math.PI) / 180; x = r * Math.sin(a); y = -r * Math.cos(a); }
    const height = ring === 1 || ring === 3;
    h += `<circle class="spk${height ? " height" : ""}${ring === 4 ? " low" : ""}" cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="${ring === 4 ? 8 : 9.5}"/>`;
    h += `<text class="lbl${height ? " height" : ""}${ring === 4 ? " low" : ""}" x="${x.toFixed(1)}" y="${y.toFixed(1)}">${ch}</text>`;
  }
  $("speakers").innerHTML = h;
}

/* ---------- inputs table ---------- */

function renderFiles() {
  const tb = $("files").tBodies[0];
  tb.innerHTML = "";
  const bad = rateMismatch();
  for (const ch of inLayout().channels) {
    const tr = tb.insertRow();
    tr.dataset.ch = ch;
    const m = meta[ch] || {};
    tr.classList.toggle("err", bad.includes(ch));
    tr.innerHTML = `<td class="ch">${ch}</td>
      <td><input type="text" readonly placeholder="drop or double-click" value="${inputs[ch] ? base(inputs[ch]).replace(/"/g, "&quot;") : ""}" title="${(inputs[ch] || "").replace(/"/g, "&quot;")}"></td>
      <td class="meta">${m.channels ? (m.channels === 1 ? "mono" : m.channels + " ch") : ""}</td>
      <td class="meta">${m.bits ? m.bits + "-bit" : ""}</td><td class="meta fs">${m.sample_rate || ""}</td><td class="meta">${fmtTime(m.duration)}</td>`;
    tr.querySelector("input").addEventListener("dblclick", () => browseFor(ch));
    tr.addEventListener("contextmenu", (e) => { e.preventDefault(); delete inputs[ch]; delete meta[ch]; renderFiles(); update(); });
  }
}

function rateMismatch() {
  if (unlocked) return [];            // unlocked mode resamples sources to the chosen rate
  const rate = curRate();
  return Object.entries(meta).filter(([, m]) => m.sample_rate && m.sample_rate !== rate).map(([c]) => c);
}

async function setFile(ch, path) {
  inputs[ch] = path;
  try { meta[ch] = await invoke("probe", { path }); } catch { meta[ch] = {}; }
  renderFiles();
  update();
}

async function browseFor(ch) {
  const p = await dialog.open({ multiple: false, filters: [{ name: "Audio", extensions: ["wav", "aif", "aiff", "flac"] }] });
  if (!p) return;
  if (ch === inLayout().channels[0]) await autoImport(p); else await setFile(ch, p);
}

/** One multichannel file for the whole layout: switch to the layout it fits and point every row at it. */
async function importWhole(path, pick, probe) {
  if (pick.layout !== curLayoutId()) {
    inputs = {}; meta = {};
    await cascade("layout", pick.layout);
  }
  for (const ch of inLayout().channels) { inputs[ch] = path; meta[ch] = probe; }
  renderFiles();
  update();
  log(`${probe.channels}-channel file imported as ${curLayout().id} (${curLayout().channels.join(" ")}).`);
}

async function autoImport(path) {
  let probe = {};
  try { probe = await invoke("probe", { path }); } catch { /* not readable: the name-based import reports it */ }
  if (probe.channels >= 2 && !srcLayoutId) {
    const pick = await invoke("multichannel_pick", { path, candidates: [curLayoutId(), ...opts.layouts.filter((l) => l !== curLayoutId())] }).catch(() => null);
    if (pick && pick.layout) {
      // a stereo file with sibling channel files is one of a set: let the names decide
      const sibs = probe.channels > 2 ? {} : await invoke("auto_import", { first: path, layout: inLayout().id }).catch(() => ({}));
      if (Object.keys(sibs).length < 2) { await importWhole(path, pick, probe); return; }
    } else if (probe.channels > 2) {
      log(`That file has ${probe.channels} channels, and this format has no layout with ${probe.channels}.`, "bad");
      return;
    }
  }
  const found = await invoke("auto_import", { first: path, layout: inLayout().id });
  const entries = Object.entries(found);
  if (!entries.length) { log("No channel ID found in that file name. Assign files row by row instead.", "bad"); return; }
  await Promise.all(entries.map(([ch, p]) => setFile(ch, p)));
  log(`Auto-import matched ${entries.length} of ${inLayout().channels.length} channels.`);
}

/* ---------- format cascade ---------- */

async function loadOptions() {
  opts = await invoke("options", { dest: $("dest").value, stream: $("stream").value, layout: curLayoutId() || "5.1", sampleRate: curRate() || 48000, unlocked });
  return opts;
}

function renderRates(want) {
  const s = curStream();
  const ds = (curDest().streams || []).find((x) => x.id === s.id);
  const allowed = ds ? ds.sample_rates : s.sample_rates;
  const rates = unlocked ? [...new Set([...s.sample_rates, ...COMMON_RATES])].sort((a, b) => a - b) : allowed;
  const items = rates.map((r) => [r, fmtRate(r)]);
  if (unlocked) items.push(["custom", "Custom…"]);
  const keep = rates.includes(want) || want === "custom" ? want : rates.includes(48000) ? 48000 : rates[0];
  fill($("rate"), items, keep);
  $("ratecustom").hidden = $("rate").value !== "custom";
}

/** The 96/24 core option exists for DTS-HD MA at 96 kHz with a core. */
function syncX96() {
  const ok = $("stream").value === "dtshd-ma" && curRate() === 96000 && $("coreOn").checked;
  $("x96Wrap").hidden = !ok;
  if (!ok) $("x96Core").checked = false;
}

function renderBitrate(from) {
  const br = $("bitrate"), o = opts;
  const lossless = o.bitrate_min == null;
  syncDtsOpts();
  const ma = $("stream").value === "dtshd-ma";
  $("coreWrap").hidden = !ma;
  // above 96 kHz the stream is the lossless extension alone (no core is written)
  const nhk = ma && curLayout() && curLayout().channels.length > 8;
  const hi = ma && (curRate() > 96000 || nhk);
  const coreBox = $("coreOn");
  if (hi && !coreBox.disabled) coreBox.dataset.was = coreBox.checked ? "1" : "0";
  if (!hi && coreBox.disabled) coreBox.checked = coreBox.dataset.was !== "0";
  coreBox.disabled = hi;
  if (hi) coreBox.checked = false;
  $("coreWrap").querySelector("span").textContent = hi ? (nhk ? "No DTS core above 8 channels: the stream is the lossless data alone" : "No DTS core above 96 kHz: the stream is the lossless data alone") : "Backward-compatible core (turn off for lossless data only)";
  syncX96();
  if (!ma) coreBox.checked = true;
  br.disabled = (lossless && !unlocked) || coreOff();
  $("brange").textContent = lossless ? (unlocked ? "free entry" : "lossless / variable") : unlocked ? `spec ${o.bitrate_min}–${o.bitrate_max}` : `${o.bitrate_min}–${o.bitrate_max}`;
  if (lossless && !unlocked) br.value = "";
  else if (from === "dest" || from === "stream" || !br.value) {
    br.value = lossless ? "" : (DEFAULT_BITRATE[$("stream").value] ?? o.bitrate_max);
    if (!lossless) br.value = Math.max(o.bitrate_min, Math.min(o.bitrate_max, br.value));
  } else if (from !== "bitrate" && !unlocked && !bitrateOk()) {
    br.value = Math.min(o.bitrate_max, Math.max(o.bitrate_min, Number(br.value)));
  }
  const chips = $("presets");
  chips.innerHTML = "";
  for (const p of o.presets) {
    const b = document.createElement("button");
    b.textContent = p;
    b.onclick = () => { br.value = p; syncChips(); update(); };
    chips.append(b);
  }
  syncChips();
}

/** DTS-HD MA with the core switched off: only the lossless extension is written (bit rate 0 tells the encoder). */
/** DTS core streams offer the rear -3 dB trim; DTS-ES adds the surround phase shift for 6.x layouts. */
function syncDtsOpts() {
  const st = $("stream").value;
  const dts = ["dts", "dts-es", "dts-96-24", "dtshd-hra"].includes(st);
  $("dtsOpts").hidden = !dts;
  $("esShiftWrap").hidden = st !== "dts-es";
  if (!dts) { $("rear3").checked = false; $("esShift").checked = false; }
  $("esNote").textContent = st === "dts-es" ? "Phase shift is recommended for 6.x Discrete, not for Matrix layouts." : "";
}

function coreOff() {
  return $("stream").value === "dtshd-ma" && !$("coreOn").checked;
}

function syncChips() {
  const v = Number($("bitrate").value);
  for (const b of $("presets").children) b.setAttribute("aria-pressed", String(Number(b.textContent) === v));
  $("bitrate").classList.toggle("bad", !bitrateOk());
}

function bitrateOk() {
  const br = $("bitrate");
  if (br.disabled) return true;
  const v = Number(br.value);
  if (!Number.isFinite(v) || v <= 0) return false;
  return unlocked || opts.bitrate_min == null || (v >= opts.bitrate_min && v <= opts.bitrate_max);
}

async function cascade(from, wantLayout) {
  if (from === "dest") {
    const list = unlocked ? cat.all_streams : curDest().streams;
    fill($("stream"), list.map((s) => [s.id, s.available ? s.name : `${s.name} (planned)`]), $("stream").value);
  }
  if (from === "dest" || from === "stream" || from === "unlock") renderRates(curRate());
  $("ratecustom").hidden = $("rate").value !== "custom";

  const keepLayout = wantLayout || curLayoutId();
  $("variants").dataset.value = keepLayout || "";
  let o = await loadOptions();
  renderLayoutPicker(o.layouts, keepLayout);
  if (curLayoutId() !== (keepLayout || "")) o = await loadOptions();
  renderBitrate(from);
  $("ext").textContent = "." + o.extension;
  drawSpeakers();
  renderFiles();
  update();
  if (from === "layout" || from === "dest" || from === "unlock") await dmLoad(true);
}

/* ---------- downmix & clip check ---------- */

let dm = { to: "stereo", matrix: null };

const fmtDb = (c) => (c.gain_db == null ? "off" : String(Math.round(c.gain_db * 100) / 100));

async function dmLoad(keepTarget = true) {
  const l = curLayout();
  const canTo51 = l.channels.filter((c) => c !== "LFE").length >= 6;
  const targets = [["stereo", "Stereo (Lo/Ro)"], ...(canTo51 ? [["5.1", "5.1"]] : [])];
  fill($("dmTarget"), targets, keepTarget ? dm.to : "stereo");
  dm.to = $("dmTarget").value;
  dm.matrix = await invoke("downmix_default", { layout: l.id, to: dm.to });
  $("dmResult").textContent = "";
  $("dmResult").className = "dm-result";
  dmRender();
}

function dmRender() {
  const m = dm.matrix, t = $("dmTable");
  t.innerHTML = `<thead><tr><th></th>${m.inputs.map((c) => `<th class="in">${c}</th>`).join("")}</tr></thead>`;
  const body = t.createTBody();
  m.outputs.forEach((out, o) => {
    const tr = body.insertRow();
    tr.innerHTML = `<td class="out">${out}</td>` + m.cells[o].map((c, i) =>
      `<td><input type="text" data-o="${o}" data-i="${i}" value="${fmtDb(c)}" class="${c.gain_db == null ? "off" : ""}" aria-label="${m.inputs[i]} into ${out}, dB"></td>`).join("");
  });
  t.querySelectorAll("input").forEach((inp) => inp.addEventListener("change", () => {
    const cell = dm.matrix.cells[inp.dataset.o][inp.dataset.i];
    const raw = inp.value.trim().toLowerCase();
    const v = Number(raw);
    if (raw === "" || raw === "off" || raw === "-inf") cell.gain_db = null;
    else if (Number.isFinite(v)) cell.gain_db = Math.max(-60, Math.min(12, v));
    inp.value = fmtDb(cell);
    inp.classList.toggle("off", cell.gain_db == null);
  }));
  $("dmNote").textContent = `${m.inputs.length} in → ${m.outputs.length} out · values in dB, "off" disconnects`;
}

async function dmCheck() {
  const need = curLayout().channels;
  const res = $("dmResult");
  if (!need.every((c) => inputs[c])) { res.className = "dm-result bad"; res.textContent = "Load all input files first."; return; }
  const entire = $("entire").checked;
  $("dmCheck").disabled = true;
  res.className = "dm-result";
  res.textContent = "Analysing…";
  try {
    const r = await invoke("downmix_check", { req: {
      layout: curLayoutId(), inputs, matrix: dm.matrix,
      start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
    }});
    const rate = curRate() || 48000;
    const peaks = r.channels.map((c, i) => `${c} ${r.peak_db[i] == null ? "silent" : r.peak_db[i].toFixed(2) + " dBFS"}`).join("   ");
    const secs = (f) => { const s = f / rate; return `${Math.floor(s / 60)}:${(s % 60).toFixed(2).padStart(5, "0")}`; };
    let txt = `${(r.frames / rate).toFixed(1)} s analysed · peaks: ${peaks}\n`;
    if (r.event_count === 0) { txt += "No clipping."; res.className = "dm-result ok"; }
    else {
      txt += `${r.event_count} clipping event${r.event_count === 1 ? "" : "s"}. First: ` +
        r.events.slice(0, 6).map((e) => `${r.channels[e.channel]} @ ${secs(e.frame)}`).join(", ") + ". Try Safe trim.";
      res.className = "dm-result bad";
    }
    res.textContent = txt;
  } catch (e) { res.className = "dm-result bad"; res.textContent = String(e); }
  $("dmCheck").disabled = false;
}

/** Dialnorm as a positive attenuation 1..=31; off always means 31 (-31 dBFS, no attenuation). */
function dialnormValue() {
  if (!$("dialnormOn").checked) return 31;
  const v = Math.abs(Math.round(Number($("dialnorm").value)));
  return Number.isFinite(v) && v >= 1 ? Math.min(v, 31) : 31;
}
function syncDialnorm() {
  const on = $("dialnormOn").checked;
  const raw = Math.abs(Math.round(Number($("dialnorm").value)));
  const bad = on && !(raw >= 1 && raw <= 31);
  $("dialnorm").classList.toggle("bad", bad);
  $("dialnormNote").textContent = !on ? "off (-31)" : bad ? "enter -1 to -31" : `-${dialnormValue()} dBFS`;
  if ($("variants").dataset.value) update();   // skip during start-up, before a layout exists
}

function update() {
  const need = inLayout().channels;
  const dnOk = !$("dialnorm").classList.contains("bad");
  const ready = need.every((c) => inputs[c]) && outDir && $("outname").value.trim() && !rateMismatch().length && bitrateOk() && dnOk && cat.has_ffmpeg && curRate() > 0;
  const available = curStream().available && (!srcLayoutId || curStream().native);
  $("go").disabled = !ready || !available;
  if (!available && srcLayoutId) log("Mixing 22.2 down needs a format with a native Subwoofa encoder.");
  else if (!available) log(`${curStream().name} has no encoder yet. It is on the roadmap; for now this format is listed so you can see what is coming.`);
  syncChips();
}

function jobSpec(inputsFor, name) {
  const entire = $("entire").checked;
  return {
    dest: $("dest").value, stream: $("stream").value, layout: curLayoutId(),
    sample_rate: curRate(),
    bitrate: coreOff() ? 0 : $("bitrate").disabled || !$("bitrate").value ? null : Number($("bitrate").value),
    dialnorm: dialnormValue(), unlocked, source_layout: srcLayoutId || null, engine: srcLayoutId || (curStream().native && (preferNative || !curStream().ffmpeg)) ? "native" : "ffmpeg", downmix: thdDownmix, channel_order: channelOrder,
    es_phase_shift: $("esShift").checked, rear_minus_3db: $("rear3").checked, core_x96: $("x96Core").checked,
    start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
    inputs: inputsFor, output: outDir + "\\" + name + $("ext").textContent,
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
  $("qCount").textContent = `${jobs.length} job${jobs.length === 1 ? "" : "s"}${active ? `, ${active} active` : ""}`;
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
      <span class="qtime">${live || done || fail ? fmtClock(j.elapsed_secs) : ""}</span>
      <span class="qbtns"></span>`;
    const btns = li.querySelector(".qbtns");
    const add = (txt, title, fn) => { const b = document.createElement("button"); b.textContent = txt; b.title = title; b.className = "plain"; b.onclick = fn; btns.append(b); };
    if (j.state === "pending" || live) add("Cancel", "Cancel this job", () => invoke("queue_cancel", { id: j.id }));
    if (!live) add("Log", "Show this job's log", async () => { try { log(await invoke("read_log", { path: j.log_path })); } catch (e) { log(String(e), "bad"); } });
    if (!live) add("✕", "Remove from the list", () => invoke("queue_remove", { id: j.id }));
    ul.append(li);
  }
}

/* ---------- presets (.swpreset, plain JSON) ---------- */

const PRESET_FILTER = [{ name: "Subwoofa preset", extensions: ["swpreset"] }];

function presetState() {
  return {
    app: "subwoofa", version: 1,
    dest: $("dest").value, stream: $("stream").value, layout: curLayoutId(), sample_rate: curRate(),
    bitrate: $("bitrate").value === "" ? null : Number($("bitrate").value),
    core_on: $("coreOn").checked, dialnorm_on: $("dialnormOn").checked, dialnorm: Number($("dialnorm").value),
    es_phase_shift: $("esShift").checked, rear_minus_3db: $("rear3").checked,
    unlocked, outname: $("outname").value, output_dir: outDir,
  };
}

async function applyPreset(p) {
  if (p.app !== "subwoofa") throw new Error("not a Subwoofa preset");
  setUnlocked(!!p.unlocked);
  if ([...$("dest").options].some((o) => o.value === p.dest)) $("dest").value = p.dest;
  await cascade("dest");
  if ([...$("stream").options].some((o) => o.value === p.stream)) $("stream").value = p.stream;
  await cascade("stream", p.layout);
  if (p.sample_rate && [...$("rate").options].some((o) => o.value === String(p.sample_rate))) {
    $("rate").value = String(p.sample_rate);
    $("ratecustom").hidden = true;
    await cascade("rate", p.layout);
  }
  $("coreOn").checked = p.core_on !== false;
  $("esShift").checked = !!p.es_phase_shift;
  $("rear3").checked = !!p.rear_minus_3db;
  $("dialnormOn").checked = !!p.dialnorm_on;
  $("dialnorm").disabled = !$("dialnormOn").checked;
  if (Number.isFinite(p.dialnorm)) $("dialnorm").value = p.dialnorm;
  syncDialnorm();
  renderBitrate("preset");
  if (p.bitrate != null && !$("bitrate").disabled) $("bitrate").value = p.bitrate;
  if (p.outname) $("outname").value = p.outname;
  if (p.output_dir) { outDir = p.output_dir; $("outdir").value = p.output_dir; }
  syncChips();
  update();
}

function setUnlocked(v) {
  unlocked = v;
  store.set("unlocked", v ? "1" : "0");
  $("optUnlocked").checked = v;
  $("unlockedTag").hidden = !v;
  $("bitrate").removeAttribute("min");
}

async function init() {
  cat = await invoke("catalog");
  // Rust serialises the enum variant (Lfe); the rest of the app uses the short ID (LFE).
  cat.layouts = cat.layouts.map((l) => ({ ...l, channels: l.channels.map((c) => (c === "Lfe" ? "LFE" : c === "Lfe2" ? "LFE2" : c)) }));

  const eng = $("engine");
  eng.classList.add(cat.has_ffmpeg ? "ok" : "bad");
  eng.lastElementChild.textContent = cat.has_ffmpeg ? "FFmpeg engine ready" : "FFmpeg not found on PATH";

  fill($("dest"), cat.destinations.map((d) => [d.id, d.name]), "generic");
  $("dialnormOn").onchange = () => { $("dialnorm").disabled = !$("dialnormOn").checked; syncDialnorm(); };
  $("dialnorm").oninput = syncDialnorm;
  syncDialnorm();
  setUnlocked(store.get("unlocked") === "1");
  await cascade("dest");

  $("dest").onchange = () => cascade("dest");
  $("stream").onchange = () => cascade("stream");
  $("rate").onchange = () => { $("ratecustom").hidden = $("rate").value !== "custom"; cascade("rate"); };
  $("ratecustom").oninput = () => cascade("rate");
  $("bitrate").oninput = () => { syncChips(); update(); };
  $("coreOn").onchange = () => { $("bitrate").disabled = coreOff(); syncX96(); syncChips(); update(); };
  $("x96Core").onchange = () => update();
  $("outname").oninput = update;
  $("entire").onchange = () => { $("start").disabled = $("duration").disabled = $("entire").checked; };
  $("importMc").onclick = async () => {
    const p = await dialog.open({ multiple: false, filters: [{ name: "Audio", extensions: ["wav", "aif", "aiff", "flac", "w64"] }] });
    if (p) await autoImport(p);
  };
  $("batch").onclick = batchFromFolder;
  $("clear").onclick = () => { inputs = {}; meta = {}; renderFiles(); update(); };
  $("srcLayout").onchange = () => { srcLayoutId = $("srcLayout").value; inputs = {}; meta = {}; renderFiles(); update(); };
  $("browse").onclick = async () => { const d = await dialog.open({ directory: true }); if (d) { outDir = d; $("outdir").value = d; update(); } };
  $("go").onclick = encode;
  $("qClear").onclick = () => invoke("queue_clear");
  const conc = Number(store.get("conc")) || 2;
  $("optConc").value = conc;
  invoke("queue_set_concurrency", { n: conc });
  $("optConc").onchange = () => { const n = Math.max(1, Math.min(8, Number($("optConc").value) || 1)); $("optConc").value = n; store.set("conc", n); invoke("queue_set_concurrency", { n }); };
  autoVerify = store.get("verify") !== "0";
  $("optVerify").checked = autoVerify;
  preferNative = store.get("native") === "1";
  $("optNative").checked = preferNative;
  $("optNative").onchange = () => { preferNative = $("optNative").checked; store.set("native", preferNative ? "1" : "0"); };
  thdDownmix = store.get("thddm") !== "0";
  $("optDownmix").checked = thdDownmix;
  $("optDownmix").onchange = () => { thdDownmix = $("optDownmix").checked; store.set("thddm", thdDownmix ? "1" : "0"); };
  channelOrder = store.get("order") || "auto";
  $("optOrder").value = channelOrder;
  $("optOrder").onchange = () => { channelOrder = $("optOrder").value; store.set("order", channelOrder); };
  $("optVerify").onchange = () => { autoVerify = $("optVerify").checked; store.set("verify", autoVerify ? "1" : "0"); };
  await window.__TAURI__.event.listen("queue", (e) => renderQueue(e.payload));
  renderQueue(await invoke("queue_list"));
  $("openSettings").onclick = () => $("settings").showModal();
  $("presetSave").onclick = async () => {
    const f = await dialog.save({ defaultPath: ($("outname").value || "preset") + ".swpreset", filters: PRESET_FILTER });
    if (!f) return;
    try { await invoke("write_text", { path: f, text: JSON.stringify(presetState(), null, 2) }); log(`Saved preset ${f}`, "ok"); }
    catch (e) { log(String(e), "bad"); }
  };
  $("presetLoad").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: PRESET_FILTER });
    if (!f) return;
    try { await applyPreset(JSON.parse(await invoke("read_log", { path: f }))); log(`Loaded preset ${f}`, "ok"); $("settings").close(); }
    catch (e) { log(String(e), "bad"); }
  };
  $("dmTarget").onchange = () => dmLoad(true);
  $("dmReset").onclick = () => dmLoad(true);
  $("dmCheck").onclick = dmCheck;
  $("dmSafe").onclick = async () => {
    const r = await invoke("downmix_safe", { matrix: dm.matrix });
    dm.matrix = r.matrix;
    dmRender();
    $("dmResult").className = "dm-result";
    $("dmResult").textContent = r.trim_db === 0 ? "Already safe: worst-case sum is at or below 1.0." : `Trimmed ${r.trim_db.toFixed(2)} dB (worst-case sum was ${r.worst_case.toFixed(3)}).`;
  };
  $("optUnlocked").onchange = async () => { setUnlocked($("optUnlocked").checked); await cascade("dest"); await cascade("unlock"); };

  await window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(async (ev) => {
    const p = ev.payload;
    const row = (pos) => { const s = window.devicePixelRatio || 1; const el = document.elementFromPoint(pos.x / s, pos.y / s); return el && el.closest("tr[data-ch]"); };
    document.querySelectorAll("tr.drop").forEach((r) => r.classList.remove("drop"));
    if (p.type === "over") { const r = row(p.position); if (r) r.classList.add("drop"); }
    if (p.type === "drop" && p.paths.length) {
      const r = row(p.position);
      if (!$("verify").hidden) setVerifyFile(p.paths[0]);
      else if (!$("tools").hidden) { tools.extra = []; openStream(p.paths[0]); }
      else if (r) await setFile(r.dataset.ch, p.paths[0]); else await autoImport(p.paths[0]);
    }
  });
}
init().catch((e) => log(String(e), "bad"));

/* ---------- StreamTools ---------- */

const tools = { path: null, info: null, extra: [] };
const STREAM_FILTERS = [{ name: "DTS / DTS-HD / TrueHD / MLP", extensions: ["dts", "dtshd", "cpt", "thd", "mlp", "truehd"] }];

function toolsLog(text, cls) {
  const el = $("tLog");
  el.textContent = text;
  el.className = "selectable" + (cls ? " " + cls : "");
}


function showInfo(i) {
  const rows = [
    ["Format", i.description],
    ["Sample rate", `${i.sample_rate} Hz`],
    ["Channels", String(i.channels || "?")],
    ["Length", `${fmtTime(i.duration_secs)}  (${i.frames.toLocaleString()} frames)`],
    ["Size", `${(i.bytes / 1e6).toFixed(1)} MB`],
    ["Bit rate", `${i.average_kbps.toFixed(0)} kbps average, ${i.peak_kbps.toFixed(0)} kbps peak`],
    ["Restart points", String(i.restart_points)],
    ["Container", i.container ? ".dtshd" : "raw stream"],
  ];
  $("tInfo").innerHTML = rows.map(([k, v]) => `<dt>${k}</dt><dd>${v.replace(/&/g, "&amp;").replace(/</g, "&lt;")}</dd>`).join("");
}

function drawPbr(points) {
  const cv = $("tPbr"), g = cv.getContext("2d");
  const W = cv.width, H = cv.height, padL = 46, padB = 18, padT = 8, padR = 8;
  g.clearRect(0, 0, W, H);
  if (!points.length) return;
  const maxK = Math.max(...points.map((p) => p.peak_kbps)) * 1.08 || 1;
  const t1 = points[points.length - 1].secs + 1;
  const x = (t) => padL + (t / t1) * (W - padL - padR);
  const y = (k) => H - padB - (k / maxK) * (H - padB - padT);
  const css = getComputedStyle(document.documentElement);
  const dim = css.getPropertyValue("--dim").trim(), lamp = css.getPropertyValue("--lamp").trim(), rule = css.getPropertyValue("--rule-hi").trim();
  g.font = "10px Cascadia Mono, Consolas, monospace";
  g.fillStyle = dim;
  g.strokeStyle = rule;
  g.lineWidth = 1;
  for (let i = 0; i <= 4; i++) {
    const k = (maxK * i) / 4, yy = y(k);
    g.beginPath(); g.moveTo(padL, yy); g.lineTo(W - padR, yy); g.stroke();
    g.fillText(k.toFixed(0), 4, yy + 3);
  }
  const line = (key, color, width) => {
    g.beginPath();
    points.forEach((p, i) => (i ? g.lineTo(x(p.secs), y(p[key])) : g.moveTo(x(p.secs), y(p[key]))));
    g.strokeStyle = color; g.lineWidth = width; g.stroke();
  };
  line("peak_kbps", dim, 1);
  line("mean_kbps", lamp, 1.5);
  g.fillStyle = dim;
  g.fillText("0", padL, H - 4);
  g.fillText(fmtTime(t1), W - padR - 40, H - 4);
}

async function openStream(path) {
  tools.path = path;
  $("tPath").value = path;
  toolsLog("Reading the stream…");
  try {
    const info = await invoke("tools_info", { path });
    tools.info = info;
    showInfo(info);
    const win = Math.max(0.5, info.duration_secs / 300);
    const pts = await invoke("tools_pbr", { path, window: win });
    drawPbr(pts);
    $("tPbrNote").textContent = `Mean (amber) and peak (grey) kbps over ${win.toFixed(1)} s windows`;
    $("tTo").placeholder = info.duration_secs.toFixed(1);
    toolsLog("Ready.");
    renderAppend();
  } catch (e) {
    tools.info = null;
    toolsLog(String(e), "bad");
  }
}

function renderAppend() {
  const list = $("tAppendList");
  list.innerHTML = "";
  const rows = [tools.path, ...tools.extra].filter(Boolean);
  rows.forEach((p, i) => {
    const li = document.createElement("li");
    li.innerHTML = `<span></span>`;
    li.firstChild.textContent = p;
    if (i > 0) {
      const b = document.createElement("button");
      b.className = "plain"; b.textContent = "Remove";
      b.onclick = () => { tools.extra.splice(i - 1, 1); renderAppend(); };
      li.append(b);
    }
    list.append(li);
  });
}

function suggestName(suffix) {
  const p = tools.path || "stream";
  const dot = p.lastIndexOf(".");
  return p.slice(0, dot) + suffix + p.slice(dot);
}

function needStream() {
  if (!tools.path || !tools.info) { toolsLog("Open a stream first.", "bad"); return false; }
  return true;
}

function setMode(mode) {
  document.body.classList.toggle("tools-mode", mode !== "encode");
  $("tools").hidden = mode !== "tools";
  $("verify").hidden = mode !== "verify";
  $("modeEncode").setAttribute("aria-pressed", String(mode === "encode"));
  $("modeTools").setAttribute("aria-pressed", String(mode === "tools"));
  $("modeVerify").setAttribute("aria-pressed", String(mode === "verify"));
}

function initTools() {
  $("modeEncode").onclick = () => setMode("encode");
  $("modeTools").onclick = () => setMode("tools");
  $("modeVerify").onclick = () => setMode("verify");
  for (const b of document.querySelectorAll(".op-tabs button")) {
    b.onclick = () => {
      for (const o of document.querySelectorAll(".op-tabs button")) o.setAttribute("aria-pressed", String(o === b));
      for (const d of document.querySelectorAll(".op")) d.hidden = d.dataset.op !== b.dataset.op;
    };
  }
  $("tOpen").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: STREAM_FILTERS });
    if (f) { tools.extra = []; openStream(f); }
  };
  $("tTrim").onclick = async () => {
    if (!needStream()) return;
    const out = await dialog.save({ defaultPath: suggestName("_trim"), filters: STREAM_FILTERS });
    if (!out) return;
    toolsLog("Cutting…");
    try {
      const to = $("tTo").value === "" ? null : Number($("tTo").value);
      toolsLog(await invoke("tools_trim", { path: tools.path, output: out, start: Number($("tFrom").value) || 0, end: to }), "ok");
    } catch (e) { toolsLog(String(e), "bad"); }
  };
  $("tSplit").onclick = async () => {
    if (!needStream()) return;
    const at = $("tAt").value.split(/[,;\s]+/).map(Number).filter((n) => Number.isFinite(n) && n > 0);
    if (!at.length) { toolsLog("Enter one or more split times in seconds.", "bad"); return; }
    const out = await dialog.save({ defaultPath: suggestName(""), filters: STREAM_FILTERS });
    if (!out) return;
    toolsLog("Splitting…");
    try {
      const files = await invoke("tools_split", { path: tools.path, output: out, at });
      toolsLog(`Wrote ${files.length} files:\n` + files.join("\n"), "ok");
    } catch (e) { toolsLog(String(e), "bad"); }
  };
  $("tAppendAdd").onclick = async () => {
    const f = await dialog.open({ multiple: true, filters: STREAM_FILTERS });
    if (f) { tools.extra.push(...(Array.isArray(f) ? f : [f])); renderAppend(); }
  };
  $("tAppendRun").onclick = async () => {
    if (!needStream()) return;
    if (!tools.extra.length) { toolsLog("Add at least one more stream.", "bad"); return; }
    const out = await dialog.save({ defaultPath: suggestName("_joined"), filters: STREAM_FILTERS });
    if (!out) return;
    toolsLog("Joining…");
    try { toolsLog(await invoke("tools_append", { inputs: [tools.path, ...tools.extra], output: out }), "ok"); }
    catch (e) { toolsLog(String(e), "bad"); }
  };
  $("tRepOpen").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: STREAM_FILTERS });
    if (f) $("tRepPath").value = f;
  };
  $("tRepRun").onclick = async () => {
    if (!needStream()) return;
    if (!$("tRepPath").value) { toolsLog("Choose the replacement stream first.", "bad"); return; }
    const out = await dialog.save({ defaultPath: suggestName("_replaced"), filters: STREAM_FILTERS });
    if (!out) return;
    toolsLog("Writing…");
    try { toolsLog(await invoke("tools_replace", { path: tools.path, with: $("tRepPath").value, at: Number($("tRepAt").value) || 0, output: out, insert: $("tRepInsert").checked }), "ok"); }
    catch (e) { toolsLog(String(e), "bad"); }
  };
  const FPS_NAMES = { 1: "23.976", 2: "24", 3: "25", 4: "29.97 drop", 5: "29.97", 6: "30 drop", 7: "30" };
  const showStripe = async () => {
    if (!tools.path) return;
    try {
      const r = await invoke("tools_stripe", { path: tools.path });
      const fps = FPS_NAMES[r.fps_code] || "?";
      const secs = r.start_samples / 48000;
      $("tStripeNow").value = `${fmtTime(secs)} at ${fps} fps`;
      if (FPS_NAMES[r.fps_code]) $("tStripeFps").value = fps;
    } catch (e) { $("tStripeNow").value = "no timecode in this file"; }
  };
  document.querySelectorAll(".op-tabs button[data-op='restripe']").forEach((b) => b.addEventListener("click", showStripe));
  $("tStripeRun").onclick = async () => {
    if (!needStream()) return;
    const out = await dialog.save({ defaultPath: suggestName("_restriped"), filters: STREAM_FILTERS });
    if (!out) return;
    toolsLog("Writing…");
    try { toolsLog(await invoke("tools_restripe", { path: tools.path, output: out, fps: $("tStripeFps").value, start: $("tStripeStart").value }), "ok"); }
    catch (e) { toolsLog(String(e), "bad"); }
  };
  $("tSilRun").onclick = async () => {
    if (!needStream()) return;
    const head = Number($("tSilHead").value) || 0, tail = Number($("tSilTail").value) || 0;
    if (head <= 0 && tail <= 0) { toolsLog("Give a head and/or tail length.", "bad"); return; }
    const out = await dialog.save({ defaultPath: suggestName("_silence"), filters: STREAM_FILTERS });
    if (!out) return;
    toolsLog("Writing…");
    try { toolsLog(await invoke("tools_add_silence", { path: tools.path, output: out, head, tail }), "ok"); }
    catch (e) { toolsLog(String(e), "bad"); }
  };
  $("tVerify").onclick = () => {
    if (tools.path) setVerifyFile(tools.path);
    setMode("verify");
  };
}
initTools();


/* ---------- Verify ---------- */

const VERIFY_FILTERS = [{ name: "Encoded audio streams", extensions: ["ac3", "eac3", "ec3", "dts", "dtshd", "cpt", "thd", "mlp", "truehd"] }, { name: "All files", extensions: ["*"] }];
const verify = { path: null, text: "", running: false };
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const STATUS_TAG = { pass: "OK", fail: "FAIL", warn: "WARN", skipped: "SKIP", info: "INFO" };

function setVerifyFile(path) {
  verify.path = path;
  $("vPath").value = path;
  $("vResult").hidden = true;
  $("vSave").disabled = true;
  vLog("Ready. Choose Verify to start.");
}

function vLog(text, cls) {
  const el = $("vLog");
  el.textContent = text;
  el.className = "selectable" + (cls ? " " + cls : "");
  el.hidden = !text;
}

function renderVerify(r) {
  const failed = r.checks.filter((c) => c.status === "fail").length + r.findings.filter((f) => f.status === "fail").length;
  const warned = r.checks.filter((c) => c.status === "warn").length + r.findings.filter((f) => f.status === "warn").length;
  const v = $("vVerdict");
  v.className = "verdict " + (r.passed ? "pass" : "fail");
  v.innerHTML = r.passed
    ? `Passed<small>${esc(r.format)}. ${r.deep ? "Deep verification" : "Structure check"} found no errors${warned ? `, ${warned} warning${warned > 1 ? "s" : ""}` : ""}.</small>`
    : `Failed<small>${esc(r.format)}. ${failed} problem${failed === 1 ? "" : "s"} found${warned ? `, ${warned} warning${warned > 1 ? "s" : ""}` : ""} - see below.</small>`;
  $("vSummary").innerHTML = r.summary.map(([k, val]) => `<dt>${esc(k)}</dt><dd>${esc(val)}</dd>`).join("");
  $("vChecks").innerHTML = r.checks.map((c) => `<li class="${c.status}"><span class="tag">${STATUS_TAG[c.status]}</span><span>${esc(c.name)}</span><span class="detail">${esc(c.detail)}</span></li>`).join("");
  $("vFindingsWrap").hidden = !r.findings.length;
  $("vFindings").innerHTML = r.findings.map((f) => `<li class="finding ${f.status}"><span class="tag">${STATUS_TAG[f.status]}</span><span class="detail">${esc(f.message)}${f.offset != null ? ` <span class="at">(byte ${f.offset.toLocaleString()})</span>` : ""}</span></li>`).join("")
    + (r.findings_omitted ? `<li class="finding info"><span class="tag">INFO</span><span class="detail">and ${r.findings_omitted.toLocaleString()} more</span></li>` : "");
  const open = new Set(["Decoded audio", "Bit rate"]);
  $("vSections").innerHTML = r.sections.map((s) => `<details${open.has(s.title) ? " open" : ""}><summary>${esc(s.title)}</summary><dl class="kv selectable">${s.rows.map(([k, val]) => `<dt>${esc(k)}</dt><dd>${esc(val)}</dd>`).join("")}</dl></details>`).join("");
  $("vResult").hidden = false;
}

function initVerify() {
  $("vOpen").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: VERIFY_FILTERS });
    if (f) setVerifyFile(f);
  };
  $("vRun").onclick = async () => {
    if (!verify.path) { vLog("Choose a file first.", "bad"); return; }
    if (verify.running) return;
    verify.running = true;
    $("vRun").disabled = true;
    $("vResult").hidden = true;
    const deep = $("vDeep").checked;
    const t0 = Date.now();
    const what = deep ? "Verifying (deep, decoding the whole file)" : "Verifying";
    const tick = setInterval(() => vLog(`${what}… ${Math.round((Date.now() - t0) / 1000)} s`), 500);
    vLog(`${what}…`);
    try {
      const out = await invoke("verify_file", { path: verify.path, deep });
      verify.text = out.text;
      renderVerify(out.report);
      $("vSave").disabled = false;
      vLog(`Done in ${((Date.now() - t0) / 1000).toFixed(1)} s.`);
    } catch (e) {
      vLog(String(e), "bad");
    } finally {
      clearInterval(tick);
      verify.running = false;
      $("vRun").disabled = false;
    }
  };
  $("vSave").onclick = async () => {
    const base = (verify.path || "report").replace(/\.[^.\\/]*$/, "");
    const f = await dialog.save({ defaultPath: base + "_verify.txt", filters: [{ name: "Text", extensions: ["txt"] }] });
    if (f) { await invoke("write_text", { path: f, text: verify.text }); vLog(`Saved ${f}`); }
  };
}
initVerify();
