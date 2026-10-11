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
const toolPaths = { ffmpeg: "", mas: "" };
let lastMode = "encode";
const TIP_BASE = {};
const TIP_EXTRA = {};

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
  syncPcmFormat();
  // no core above 8 channels, or above 6 channels over 96 kHz: the stream is the lossless extension alone
  const nch = curLayout() ? curLayout().channels.length : 0;
  const nhk = ma && nch > 8;
  const hi = ma && (nhk || (curRate() > 96000 && nch > 6));
  const coreBox = $("coreOn");
  if (hi && !coreBox.disabled) coreBox.dataset.was = coreBox.checked ? "1" : "0";
  if (!hi && coreBox.disabled) coreBox.checked = coreBox.dataset.was !== "0";
  coreBox.disabled = hi;
  if (hi) coreBox.checked = false;
  const coreLabel = $("coreWrap").querySelector("span");
  coreLabel.firstChild.textContent = hi ? "No DTS core here" : "Backward-compatible core";
  setTip("#coreOn", hi ? (nhk ? "Above 8 channels there is no DTS core: the stream is the lossless data alone." : "Above 96 kHz with more than 6 channels there is no DTS core: the stream is the lossless data alone.") : "");
  syncX96();
  if (!ma) coreBox.checked = true;
  br.disabled = (lossless && !unlocked) || coreOff();
  const range = lossless ? (unlocked ? "Free entry here." : "Lossless and variable, so there is nothing to set.") : `This format takes ${o.bitrate_min} to ${o.bitrate_max} kbps${unlocked ? " by the spec; anything goes while limits are off" : ""}.`;
  $("brange").textContent = "";
  setTip("#bitrate", range);
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
  setTip("#dtsOpts", st === "dts-es" ? "On 6.x Discrete the phase shift is recommended. On Matrix layouts, leave it be." : "");
}

/** One plain sentence about what the chosen stream type puts in the file, so the options are not a mystery. */
function describeStream() {
  const st = $("stream").value, lay = curLayout(), rate = curRate();
  const nch = lay ? lay.channels.length : 0;
  const total = Number($("bitrate").value);
  let t = "";
  if (st === "dtshd-hra") {
    const rest = Math.max(0, total - 1509);
    if (nch > 6 && rate > 48000) {
      t = `A 1509 kbps 48 kHz core (5.1 downmix) for older players, an XXCH part for the rear pair (about ${Math.round(rest * 0.2)} kbps) and an X96 part for the 96 kHz band of every channel (about ${Math.round(rest * 0.8)} kbps).`;
    } else if (rate > 48000) {
      const six = nch - (lay.channels.includes("LFE") ? 1 : 0) === 6;
      t = `A 1509 kbps 48 kHz core for older players${six ? " (the centre surround folded into the surrounds, and also sent separately for DTS-ES decoders when the layout is Discrete)" : ""}, plus an X96 part for the 96 kHz band of every channel (about ${rest} kbps).`;
    } else if (nch > 6) {
      t = `A 1509 kbps core (5.1 downmix) for older players, plus an XXCH part for the rear pair and an XBR part with extra detail (together about ${rest} kbps).`;
    } else {
      t = `A 1509 kbps core, plus an XBR part (about ${rest} kbps) that adds detail the core left out.`;
    }
  } else if (st === "dtshd-ma") {
    t = coreOff() ? "Lossless audio only: players that can't decode DTS-HD MA won't play it." : "Lossless audio, with a lossy core inside it so older DTS players still play the stream.";
  } else if (st === "dts-96-24") {
    t = "A 48 kHz core plus an X96 part: core-only decoders play it at 48 kHz, 96/24 decoders get the full band.";
  } else if (st === "dts-es") {
    t = "A DTS core with a sixth surround channel, matrixed into the surrounds or sent separately (Discrete).";
  } else if (st === "dts") {
    t = "A plain DTS core: lossy, plays everywhere.";
  }
  setTip("#stream", t);
  updatePlate();
}

/** Audio files (WAV, AIFF, CAF, FLAC) have a sample format instead of a bit rate and a dialnorm. */
function syncPcmFormat() {
  const st = $("stream").value;
  const file = ["wav", "aiff", "caf", "flac", "oggflac", "alac", "tta", "dsf", "dff"].includes(st);
  $("pcmWrap").hidden = !file;
  document.querySelector(".f.dialnorm").hidden = file; // no dialogue level in a plain audio file
  if (!file) return;
  const dsd = st === "dsf" || st === "dff";
  const flac = st === "flac" || st === "oggflac" || st === "alac" || st === "tta";
  // DSD offers only the DSD rates; the integer-only formats only 16 and 24 bit; the rest no DSD
  const allowed = (v) => dsd ? v.startsWith("dsd") : v.startsWith("dsd") ? false : flac ? ["s16", "s24"].includes(v) : true;
  for (const o of $("pcmFormat").options) o.disabled = !allowed(o.value);
  if ($("pcmFormat").selectedOptions[0].disabled) $("pcmFormat").value = dsd ? "dsd64" : "s24";
  setTip("#pcmFormat", dsd
    ? "One-bit DSD made with a fifth-order sigma-delta modulator at half scale (-6 dB), the filters keeping the band up to about 20 kHz (40 kHz from 88.2 kHz sources). Audio only: no disc image."
    : st === "tta"
    ? "TTA stores 16 or 24 bit integers."
    : st === "alac"
    ? "Apple Lossless stores 16 or 24 bit integers, up to 8 channels in Apple's arrangements (7.1 needs the wide layout)."
    : flac
    ? "FLAC stores 16 or 24 bit integers, up to 8 channels."
    : st === "aiff"
      ? "AIFF files are limited to 4 GB; floating point makes it an AIFF-C file."
      : st === "wav"
        ? "Files over 4 GB are written as RF64. The channel mask is set where the layout has standard speaker positions."
        : "CAF has no 4 GB limit and labels every channel.");
}

function coreOff() {
  return $("stream").value === "dtshd-ma" && !$("coreOn").checked;
}

function syncChips() {
  describeStream();
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
  syncBitstream();
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

const DEST_TAG = { "bluray-primary": "BD", "bluray-secondary": "BD Secondary", dvd: "DVD", "digital-delivery": "Digital", "dts-music-disc": "DTS Disc" };
const STREAM_TAG = { "dtshd-ma": "DTSHDMA", "dtshd-hra": "DTSHDHRA", "dts-es": "DTSES", "dts-96-24": "DTS9624", dts: "DTS", "dts-express": "DTSExpress", ac3: "AC3", eac3: "EAC3", truehd: "TrueHD", mlp: "MLP", wav: "WAV", aiff: "AIFF", caf: "CAF", flac: "FLAC", oggflac: "OggFLAC", alac: "ALAC", tta: "TTA", dsf: "DSD", dff: "DSD" };
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
    pcm_format: ["wav", "aiff", "caf", "flac", "oggflac", "alac", "tta", "dsf", "dff"].includes($("stream").value) ? $("pcmFormat").value : "", es_phase_shift: $("esShift").checked, rear_minus_3db: $("rear3").checked, core_x96: $("x96Core").checked,
    start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
    inputs: inputsFor, output: outDir + "\\" + name + $("ext").textContent,
    bitstream: bitstreamSpec(),
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


/* ---------- Bitstream tab ---------- */

let bsCsvPath = "";
let bsCsvPoints = [];

/** Which parts of the Bitstream tab make sense for the chosen format and layout. */
function syncBitstream() {
  const st = $("stream").value, dest = $("dest").value, lay = curLayoutId();
  const dts = ["dts", "dts-es", "dts-96-24", "dtshd-hra", "dtshd-ma", "dts-express"].includes(st);
  const container = $("ext").textContent === ".dtshd";
  const secondary = st === "dts-express" && dest === "bluray-secondary";
  const express = st === "dts-express";
  $("bsInfoWrap").hidden = !container || !dts;
  $("bsTcWrap").hidden = !container || !dts;
  $("bsSeamlessWrap").hidden = !container || !dts || express;
  $("bsWideWrap").hidden = !(lay === "7.1-wide" && ["dtshd-ma", "dtshd-hra"].includes(st));
  $("bsDialogWrap").hidden = !express;
  $("bsSecWrap").hidden = !secondary;
  $("bsMapWrap").hidden = !(secondary && (curLayout() ? curLayout().channels.length : 0) === 1);
  $("bs").hidden = [...$("bs").querySelectorAll(".bs-grid > .f")].every((n) => n.hidden);
  $("bsCsv").hidden = $("bsSeamless").value !== "csv";
  $("bsCsvNote").textContent = $("bsSeamless").value === "csv" ? (bsCsvPath ? `${bsCsvPoints.length} branch point${bsCsvPoints.length === 1 ? "" : "s"} from ${bsCsvPath.split(/[\\/]/).pop()}` : "no CSV file chosen") : "";
  syncDownmix();
  syncTabs();
}

const DMX_CH = ["C", "L", "R", "Ls", "Rs", "LFE"];
const DMX_DEFAULT = {
  primary: [3, 3, 3, 3, 3, 3],
  extra_a: [null, null, null, 3, null, null], extra_b: [null, null, null, null, 3, null],
  stereo_a: [6, 3, null, 6, null, null], stereo_b: [6, null, 3, null, 6, null],
};
const DMX_TABLES = {
  dmx51Grid: { key: "dmx51", heads: ["5.1 level", "Extra 1", "Extra 2"], cols: ["primary", "extra_a", "extra_b"] },
  dmx20Grid: { key: "dmx20", heads: ["To left", "To right"], cols: ["stereo_a", "stereo_b"] },
};

function buildDmxGrids() {
  for (const [id, t] of Object.entries(DMX_TABLES)) {
    const grid = $(id);
    grid.style.gridTemplateColumns = `3.2em repeat(${t.cols.length}, 6.5em)`;
    grid.innerHTML = `<span></span>` + t.heads.map((h) => `<span class="h">${h}</span>`).join("") +
      DMX_CH.map((ch, i) => `<span>${ch}</span>` + t.cols.map((c) => `<input type="number" min="0" max="90" step="0.5" id="${t.key}_${c}_${i}" aria-label="${ch}, ${t.heads[t.cols.indexOf(c)]}" value="${DMX_DEFAULT[c][i] ?? ""}">`).join("")).join("");
  }
}

function dmxTable(col) {
  return DMX_CH.map((_, i) => {
    const v = $(`${col.startsWith("stereo") ? "dmx20" : "dmx51"}_${col}_${i}`).value.trim();
    return v === "" ? null : Math.max(0, Math.min(90, Number(v)));
  });
}

/** The Downmix settings in the shape the encoder takes them. */
function dmxSpec() {
  const stereo = !$("dmxStereoWrap").hidden;
  return {
    primary: dmxTable("primary"), extra_a: dmxTable("extra_a"), extra_b: dmxTable("extra_b"), stereo_a: dmxTable("stereo_a"), stereo_b: dmxTable("stereo_b"),
    embed_stereo: stereo && !$("dmxEmbedWrap").hidden && $("dmxEmbed").checked,
    stereo_matrix: stereo && !$("dmxMatrixWrap").hidden && $("dmxMatrix").checked,
    lt_rt: stereo && $("dmxLtRt").checked,
  };
}

function setDmx(d) {
  d = d || {};
  for (const col of ["primary", "extra_a", "extra_b", "stereo_a", "stereo_b"]) {
    const vals = d[col] || DMX_DEFAULT[col];
    DMX_CH.forEach((_, i) => { $(`${col.startsWith("stereo") ? "dmx20" : "dmx51"}_${col}_${i}`).value = vals[i] ?? ""; });
  }
  $("dmxEmbed").checked = !!d.embed_stereo; $("dmxMatrix").checked = !!d.stereo_matrix; $("dmxLtRt").checked = !!d.lt_rt;
}

/** Which parts of the Downmix section make sense for the chosen format and layout. */
function syncDownmix() {
  const st = $("stream").value, lay = curLayout();
  const ch = lay ? lay.channels : [];
  const has = (...c) => c.every((x) => ch.includes(x));
  const five = has("C", "L", "R") && (has("Ls", "Rs") || has("Lss", "Rss"));
  const lfe = ch.includes("LFE") ? 1 : 0;
  const ma = st === "dtshd-ma", hra = st === "dtshd-hra";
  const esMatrix = lay && lay.id.startsWith("6.") && !lay.id.endsWith("premixed");
  const pair = five && ch.length === 7 + lfe;
  // Master Audio can carry a stereo set and the levels; the core streams only write the levels (into their auxiliary data)
  const coreLevels = ["dts", "dts-96-24", "dtshd-hra"].includes(st) && five && ch.length <= 8 && !esMatrix;
  const stereoOk = (ma && five && ch.length <= 8) || coreLevels;
  $("dmxStereoWrap").hidden = !stereoOk;
  $("dmxEmbedWrap").hidden = !(ma && stereoOk && curRate() <= 96000);
  $("dmxMatrixWrap").hidden = !(stereoOk && !esMatrix);
  $("dmxLtRtWrap").hidden = $("dmxMatrixWrap").hidden;
  $("dmx51Wrap").hidden = !((ma || hra) && pair);
  $("dmx20Wrap").hidden = $("dmxStereoWrap").hidden || !(($("dmxEmbed").checked && !$("dmxEmbedWrap").hidden) || ($("dmxMatrix").checked && !$("dmxMatrixWrap").hidden));
  // High Resolution Audio scales every core channel by one value
  for (let i = 1; i < 6; i++) {
    const cell = $(`dmx51_primary_${i}`);
    cell.disabled = hra;
    if (hra) cell.value = $("dmx51_primary_0").value;
  }
  $("dmx").hidden = $("dmxStereoWrap").hidden && $("dmx51Wrap").hidden;
}

/** Dropdown level (dB below full) of a speaker box; empty means off. */
function bsLevel(id) {
  const v = $(id).value.trim();
  return v === "" ? null : Math.max(0, Math.min(60, Number(v)));
}

/** The Bitstream settings in the shape the encoder takes them. */
function bitstreamSpec() {
  const seam = $("bsSeamless").value;
  return {
    program_info: $("bsInfoWrap").hidden ? "" : $("bsInfo").value,
    seamless: $("bsSeamlessWrap").hidden || seam === "off" ? "Off" : seam === "single" ? "SingleClip" : { Timecodes: bsCsvPoints },
    wide_remap: !$("bsWideWrap").hidden && $("bsWide").checked,
    downmix: dmxSpec(),
    timecode: $("bsTcWrap").hidden ? { rate: "", start: "", reference: "" } : { rate: $("bsFps").value, start: $("bsTcStart").value.trim(), reference: $("bsTcRef").value.trim() },
    secondary: {
      attenuation_db: Math.max(0, Math.min(60, Number($("bsAtt").value) || 0)),
      fade_down_secs: Math.max(0, Math.min(9, Number($("bsFadeDown").value) || 0)),
      fade_up_secs: Math.max(0, Math.min(9, Number($("bsFadeUp").value) || 0)),
      dialog_mode: !$("bsDialogWrap").hidden && $("bsDialog").checked,
      mono_map: { c: bsLevel("bsMapC"), l: bsLevel("bsMapL"), r: bsLevel("bsMapR"), ls: bsLevel("bsMapLs"), rs: bsLevel("bsMapRs"), lfe: bsLevel("bsMapLfe") },
    },
  };
}

async function chooseBranchCsv() {
  const path = await dialog.open({ multiple: false, filters: [{ name: "CSV branch points", extensions: ["csv", "txt"] }] });
  if (!path) return;
  try {
    const text = await invoke("read_log", { path });
    bsCsvPoints = text.split(/\r?\n/).map((l) => l.split(",")[0].trim()).filter(Boolean);
    bsCsvPath = path;
  } catch (e) { log(String(e), "bad"); }
  syncBitstream();
}

/* ---------- themes ---------- */

const THEMES = [
  ["amber", "Studio Amber"], ["graphite", "Graphite"], ["midnight", "Midnight"], ["ocean", "Ocean"], ["moss", "Moss"],
  ["rose", "Rose Dusk"], ["terminal", "Terminal"], ["paper", "Paper"], ["daylight", "Daylight"], ["contrast", "High Contrast"],
];
const CUSTOM_KEYS = ["ground", "panel", "rule", "ink", "dim", "lamp"];

/** The six colours a theme block gives, read back from the stylesheet. */
function themeColours(name) {
  const probe = document.createElement("div");
  probe.dataset.theme = name;
  probe.style.display = "none";
  document.body.append(probe);
  const cs = getComputedStyle(probe);
  const out = {};
  for (const k of CUSTOM_KEYS) out[k] = cs.getPropertyValue("--" + k).trim();
  probe.remove();
  return out;
}

function luminance(hex) {
  const n = parseInt(hex.slice(1), 16), c = [n >> 16, (n >> 8) & 255, n & 255].map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

/** Everything else a theme needs, worked out from the six colours. */
function customVars(c) {
  const light = luminance(c.ground) > 0.4;
  return {
    "--ground": c.ground, "--panel": c.panel, "--rule": c.rule, "--ink": c.ink, "--dim": c.dim, "--lamp": c.lamp,
    "--hi": "color-mix(in srgb, var(--panel) 90%, var(--ink))",
    "--screen": light ? "color-mix(in srgb, var(--panel) 55%, #fff)" : "color-mix(in srgb, var(--ground) 65%, #000)",
    "--rule-hi": "color-mix(in srgb, var(--rule) 65%, var(--ink))",
    "--lamp-ink": luminance(c.lamp) > 0.35 ? "#14110a" : "#ffffff",
    "color-scheme": light ? "light" : "dark",
  };
}

function loadCustom() {
  try { const c = JSON.parse(store.get("customTheme") || "null"); if (c && c.colors) return c; } catch { /* fall through */ }
  return { base: "amber", colors: themeColours("amber") };
}

function saveCustom(c) {
  c.vars = customVars(c.colors);
  store.set("customTheme", JSON.stringify(c));
  return c;
}

function clearInlineTheme() {
  const s = document.documentElement.style;
  for (const k of ["--ground", "--panel", "--rule", "--ink", "--dim", "--lamp", "--hi", "--screen", "--rule-hi", "--lamp-ink", "color-scheme"]) s.removeProperty(k);
}

function applyTheme(name) {
  if (name !== "custom" && !THEMES.some((t) => t[0] === name)) name = "amber";
  clearInlineTheme();
  document.documentElement.dataset.theme = name;
  if (name === "custom") for (const [k, v] of Object.entries(saveCustom(loadCustom()).vars)) document.documentElement.style.setProperty(k, v);
  store.set("theme", name);
  for (const b of $("themeGrid").children) b.setAttribute("aria-checked", String(b.dataset.theme === name));
  $("customEditor").hidden = name !== "custom";
}

function paintCustomCard() {
  const card = $("themeGrid").querySelector('[data-theme="custom"]');
  if (!card) return;
  const c = loadCustom();
  for (const [k, v] of Object.entries(customVars(c.colors))) card.style.setProperty(k, v);
}

function buildThemeGrid() {
  const all = [...THEMES, ["custom", "Custom"]];
  $("themeGrid").innerHTML = all.map(([id, label]) =>
    `<button type="button" role="radio" class="theme" data-theme="${id}" aria-checked="false"><span class="sw"><i></i><b></b><u></u><s></s></span><span class="n">${label}</span></button>`).join("");
  $("themeGrid").onclick = (e) => { const b = e.target.closest(".theme"); if (b) { applyTheme(b.dataset.theme); syncCustomEditor(); } };
  fill($("customBase"), THEMES.map(([id, label]) => [id, label]), loadCustom().base);
  paintCustomCard();
  applyTheme(document.documentElement.dataset.theme);
  syncCustomEditor();
  for (const input of $("customEditor").querySelectorAll("input[type=color]")) {
    input.oninput = () => {
      const c = loadCustom();
      c.colors[input.dataset.var] = input.value;
      saveCustom(c);
      applyTheme("custom");
      paintCustomCard();
    };
  }
  $("customBase").onchange = () => resetCustom($("customBase").value);
  $("customReset").onclick = () => resetCustom($("customBase").value);
  $("customCopy").onclick = async () => {
    try { await navigator.clipboard.writeText(JSON.stringify(loadCustom().colors)); flash($("customCopy"), "Copied"); } catch { flash($("customCopy"), "Blocked"); }
  };
  $("customPaste").onclick = async () => {
    try {
      const t = JSON.parse(await navigator.clipboard.readText());
      const c = loadCustom();
      for (const k of CUSTOM_KEYS) if (/^#[0-9a-f]{6}$/i.test(t[k] || "")) c.colors[k] = t[k];
      saveCustom(c); applyTheme("custom"); syncCustomEditor(); paintCustomCard();
      flash($("customPaste"), "Pasted");
    } catch { flash($("customPaste"), "No theme text"); }
  };
}

function resetCustom(base) {
  const c = { base, colors: themeColours(base) };
  saveCustom(c);
  applyTheme("custom");
  syncCustomEditor();
  paintCustomCard();
}

function syncCustomEditor() {
  const c = loadCustom();
  for (const input of $("customEditor").querySelectorAll("input[type=color]")) input.value = c.colors[input.dataset.var];
  $("customBase").value = c.base;
}

function flash(btn, text) {
  const was = btn.textContent;
  btn.textContent = text;
  setTimeout(() => { btn.textContent = was; }, 1200);
}

/* ---------- tabs of the main card ---------- */

function showTab(id) {
  for (const p of ["inputsPanel", "bs", "dmx"]) $(p).classList.toggle("active", p === id);
  for (const b of $("mainTabs").children) b.setAttribute("aria-pressed", String(b.dataset.tab === id));
}

/** A tab is offered only while its panel applies to the chosen format and layout. */
function syncTabs() {
  let active = [...$("mainTabs").children].find((b) => b.getAttribute("aria-pressed") === "true")?.dataset.tab || "inputsPanel";
  for (const b of $("mainTabs").children) b.hidden = b.dataset.tab !== "inputsPanel" && $(b.dataset.tab).hidden;
  $("mainTabs").hidden = $("bs").hidden && $("dmx").hidden;
  if ($(active).hidden) active = "inputsPanel";
  showTab(active);
}

/* ---------- (i) marks ---------- */

// [selector, text]. A selector starting with @ names the label itself; any other names a control, and the mark
// goes after its label. Anything that changes with the choices (what a stream type contains, for one) is added
// to the end with setTip.
const INFO = [
  ["@.format .dh", "Everything about the stream itself: format, rate, bit rate and the loudness tag."],
  ["#dest", "Where this file is going. It sets the rules: which formats, rates and layouts are allowed. Blu-ray and DVD are strict, Generic is relaxed, and Audio files will take nearly anything."],
  ["#stream", "The audio format. Lossless keeps every bit; lossy keeps what you probably won't miss. Pick whatever your disc or player asks for."],
  ["#rate", "Samples per second. Match your source: 48 kHz for video, 96 kHz and up for the audiophile corner. A source at another rate gets converted."],
  ["#bitrate", "Kilobits per second for the lossy part. More sounds better and weighs more. The range beside the label is what this format allows; the buttons are the usual suspects."],
  ["#x96Core", "Adds an extension so a decoder that only reads the core still gets the full 96 kHz band."],
  ["#coreOn", "The lossy core lets players without DTS-HD decode something. Switch it off for lossless data only: smaller, but picky players may stay silent."],
  ["#dialnormOn", "Dialogue normalisation: the average loudness of your speech, stored so players can level different titles. Don't know your figure? Leave it off. Film mixes often sit around −27."],
  ["#dtsOpts", "ES phase shift keeps the rear centre from cancelling against the other rears. The −3 dB option lowers the rears to match a matrixed surround."],
  ["#pcmFormat", "How each sample is stored in the file: integer or float, 16 to 64 bits, or one-bit DSD for the brave."],
  ["@.layouts .dh", "The speaker arrangement. Pick a family like 5.1, then the exact variant. The display shows who plays where, and your input files follow the same layout."],
  ["@#inputsPanel .inputs-head h2", "One mono file per channel. Drop the left file anywhere and Subwoofa finds its siblings, or import one multichannel file and watch every row fill itself."],
  ["#srcLayout", "For when your files don't match the output layout. NHK 22.2 sources get mixed down to whatever you picked."],
  ["#entire", "Untick to encode just a slice: choose where it starts and how long it lasts. Good for a quick test before you commit an hour of audio."],
  ["@#dm summary", "See what a downmix does to your levels before you ship it. Check clipping tells you whether the worst case goes over full scale; Safe trim turns it down just enough."],
  ["#bsInfo", "Free text stored in the .dtshd file: a title, notes, a message to future you. Nobody hears it."],
  ["#bsFps", "The video frame rate and start time stored in the file, so authoring tools place the audio exactly. The reference time is where the programme was cut from."],
  ["#bsSeamless", "Lets a Blu-ray player hop between versions of a film without a hiccup, by making the stream restart cleanly at chosen points. Several branch points? Give a CSV of timecodes, one per line."],
  ["#bsWide", "Adds the data that lets a decoder play wide front channels on a side-rear layout or a plain 5.1 system."],
  ["#bsDialog", "Codes only the lower half of the frequency range. Plenty for speech, and it saves bits."],
  ["#bsAtt", "How far the main film audio is turned down while this secondary track speaks."],
  ["#bsFadeDown", "Seconds the main audio takes to sink to that lower level when the commentary starts."],
  ["#bsFadeUp", "Seconds the main audio takes to come back when the commentary ends."],
  ["@#bsMapWrap .cap", "Where a mono commentary plays, in dB below full level. Empty means off. Centre at 0 is the sensible default."],
  ["#dmxEmbed", "Puts a ready-made stereo mix inside the file, so a stereo player doesn't have to fold the surround down itself. The file gets bigger."],
  ["#dmxMatrix", "Stores your stereo mixing levels in the stream as information. Decoders may use them when they downmix."],
  ["#dmxLtRt", "Labels the stereo mix as Lt/Rt (surround-encoded, for a matrix decoder) instead of plain Lo/Ro."],
  ["@#dmx51Wrap > label", "How a 7.x stream plays as 5.1. The first column is the gain of each 5.1 channel; Extra 1 and Extra 2 say how much of the two extra channels joins it. Numbers are dB below full level, and empty means not mixed in."],
  ["@#dmx20Wrap > label", "How the stream plays as stereo: how much of each channel goes to the left and right outputs, in dB below full level. Empty means none."],
  ["@.queue-head h2", "Encodes wait here, run in turn and get verified. Close the app and they go away."],
  ["@.tools-info .dh", "What's inside the stream. Handy for answering \"what even is this file?\""],
  ["@.op[data-op=trim] .op-title", "Keeps the part between two times, cut on frame boundaries. TrueHD and MLP start at the nearest earlier restart point, because that's how they are."],
  ["@.op[data-op=split] .op-title", "Cuts the stream at each time and writes numbered files next to the name you choose."],
  ["@.op[data-op=append] .op-title", "Joins streams of the same format and sample rate end to end. The open stream goes first."],
  ["@.op[data-op=replace] .op-title", "Swaps a stretch of the open stream for another one (same format, rate and channels) starting at the time you give, or inserts it without removing anything. Cuts land on frames a decoder can start from."],
  ["@.op[data-op=restripe] .op-title", "Relabels the start time and frame rate in a .dtshd. The audio isn't touched, and the reference time stays put. Needs a timecode chunk, which Subwoofa and the DTS encoder both write."],
  ["@.op[data-op=silence] .op-title", "Adds whole frames of silence to the start and/or end of a .dtshd, so the length lands within one frame of what you asked. Needs a stored silent frame."],
  ["@.op[data-op=verify] .op-title", "Checks the open stream: structure and checksums. Deep mode adds a full decode and a look at every header."],
  ["#vDeep", "Also decodes the whole file, measures the audio, asks DTS's own verifier if it's installed, and lists every header. Slower, more suspicious."],
  ["@#atmosTitle", "Atmos is encoded by the Dolby Encoding Engine, a copy you're licensed to use; Subwoofa only drives it and checks what comes out. Masters must come from Dolby's tools (DAMF .atmos, or an ADM BWF .wav), because the engine refuses anything else. A plain channel bed (.wav, up to 7.1.4) makes Dolby Digital Plus Atmos."],
  ["#aDialnorm", "Average speech loudness, −31 to −1. Leave it at 0 and the engine measures it for you."],
  ["#optUnlocked", "FOR ADVANCED USERS ONLY. Switches off the spec checks. Any rate, any layout, any format to any destination. The encoder may say no, and real hardware may too."],
  ["#optVerify", "After each encode: lossless files are decoded and compared with the source (MD5); lossy ones must decode cleanly; audio files get a structure check and a full decode."],
  ["#optNative", "Where Subwoofa has its own encoder (AC-3, E-AC-3, TrueHD, MLP, the DTS family), use it instead of FFmpeg. Formats FFmpeg can't make always use ours."],
  ["#optDownmix", "Native TrueHD only: adds a stereo substream so stereo-only players get a proper downmix. Costs roughly 5 to 8 % more file."],
  ["#optConc", "How many encodes run at once. More is faster on a big machine and louder on a small one."],
  ["#optOrder", "Some multichannel files list their channels in an unexpected order. Auto uses film order for 7.1; the others follow the file's speaker mask, or the order of the layout."],
  ["@.engine:nth-of-type(1) .wordmark", "Reads and converts audio Subwoofa can't read itself, and makes the formats it doesn't encode natively. Leave this on automatic to search your PATH, or point it at ffmpeg.exe (or its folder) to use a particular build."],
  ["@.engine:nth-of-type(2) .wordmark", "The folder that holds DTSHDVerify.exe. Subwoofa uses DTS's own verifier as a second opinion in deep checks. Automatic looks in the default install folder."],
  ["@.engine:nth-of-type(3) .wordmark", "dee.exe, the Dolby Encoding Engine, which does the actual Atmos encoding. You supply your own licensed copy."],
  ["@.spage[data-s=appearance] .sh", "Pick a look, or mix your own under Custom. Your choice is remembered."],
  ["@#customEditor .sub", "Six colours are enough. The rest (raised panels, screens, outlines, the text on the accent) is worked out from them."],
  ["@.spage[data-s=presets] .sh", "A preset remembers format, layout, rate, bit rate, dialnorm and every Bitstream and Downmix choice. Not your input files."],
];


function composeTip(key) {
  const x = TIP_EXTRA[key];
  return TIP_BASE[key] + (x ? " " + x : "");
}

/** Add to the explanation behind a control's (i) (it may change as you pick things). */
function setTip(key, text) {
  TIP_EXTRA[key] = text;
  const icon = document.querySelector(`.info[data-key="${key}"]`);
  if (icon && TIP_BASE[key] !== undefined) icon.dataset.tip = composeTip(key);
}

function addInfos() {
  for (const [sel, text] of INFO) {
    let host;
    if (sel.startsWith("@")) host = document.querySelector(sel.slice(1));
    else {
      const el = document.querySelector(sel);
      const f = el && el.closest(".f, .chk, .opt, .srow, label");
      host = f && f.querySelector(":scope > span > b, :scope > b, :scope > span, :scope > label > span, span");
    }
    if (!host || host.querySelector(".info")) continue;
    TIP_BASE[sel] = text;
    const i = document.createElement("i");
    i.className = "info"; i.tabIndex = 0; i.textContent = "i"; i.dataset.key = sel; i.dataset.tip = composeTip(sel);
    i.setAttribute("aria-label", "More information");
    host.append(i);
  }
}

function initTips() {
  const tip = $("tip");
  const show = (el) => {
    tip.textContent = el.dataset.tip;
    tip.classList.add("on");
    const r = el.getBoundingClientRect(), t = tip.getBoundingClientRect();
    const x = Math.max(8, Math.min(r.left + r.width / 2 - t.width / 2, innerWidth - t.width - 8));
    const y = r.bottom + 8 + t.height > innerHeight ? r.top - t.height - 8 : r.bottom + 8;
    tip.style.left = x + "px"; tip.style.top = Math.max(8, y) + "px";
  };
  const hide = () => tip.classList.remove("on");
  document.addEventListener("mouseover", (e) => { const el = e.target.closest?.(".info"); if (el) show(el); });
  document.addEventListener("mouseout", (e) => { if (e.target.closest?.(".info")) hide(); });
  document.addEventListener("focusin", (e) => { const el = e.target.closest?.(".info"); if (el) show(el); });
  document.addEventListener("focusout", hide);
  document.addEventListener("scroll", hide, true);
  // a path box opens its chooser on a double click, as the file rows already do
  document.addEventListener("dblclick", (e) => {
    const box = e.target.closest?.("input[data-opens]");
    if (box) $(box.dataset.opens).click();
  });
}

/* ---------- format and destination plates ---------- */



function setPlate(el, file, alt) {
  el.innerHTML = file ? `<img src="logos/${file}" alt="">` : "";
  if (file) el.firstChild.alt = alt;
}

function updatePlate() {
  const st = $("stream").value;
  setPlate($("fmtPlate"), st ? `${st}.svg` : "", $("stream").selectedOptions[0]?.textContent || "");
  const d = $("dest").value;
  setPlate($("destPlate"), d ? `dest-${d}.svg` : "", $("dest").selectedOptions[0]?.textContent || "");
}

/* ---------- presets (.swpreset, plain JSON) ---------- */

const PRESET_FILTER = [{ name: "Subwoofa preset", extensions: ["swpreset"] }];

function presetState() {
  return {
    app: "subwoofa", version: 1,
    dest: $("dest").value, stream: $("stream").value, layout: curLayoutId(), sample_rate: curRate(),
    bitrate: $("bitrate").value === "" ? null : Number($("bitrate").value),
    pcm_format: $("pcmFormat").value, core_on: $("coreOn").checked, dialnorm_on: $("dialnormOn").checked, dialnorm: Number($("dialnorm").value),
    es_phase_shift: $("esShift").checked, rear_minus_3db: $("rear3").checked,
    bitstream: { fps: $("bsFps").value, tc_start: $("bsTcStart").value, tc_ref: $("bsTcRef").value, info: $("bsInfo").value, seamless: $("bsSeamless").value, csv_path: bsCsvPath, wide: $("bsWide").checked, dialog: $("bsDialog").checked,
      att: $("bsAtt").value, fade_down: $("bsFadeDown").value, fade_up: $("bsFadeUp").value,
      map: { c: $("bsMapC").value, l: $("bsMapL").value, r: $("bsMapR").value, ls: $("bsMapLs").value, rs: $("bsMapRs").value, lfe: $("bsMapLfe").value } },
    downmix: dmxSpec(),
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
  const bs = p.bitstream || {};
  $("bsInfo").value = bs.info || "";
  $("bsFps").value = bs.fps || ""; $("bsTcStart").value = bs.tc_start || ""; $("bsTcRef").value = bs.tc_ref || "";
  $("bsSeamless").value = ["off", "single", "csv"].includes(bs.seamless) ? bs.seamless : "off";
  $("bsWide").checked = !!bs.wide;
  $("bsDialog").checked = !!bs.dialog;
  $("bsAtt").value = bs.att ?? 6; $("bsFadeDown").value = bs.fade_down ?? 0; $("bsFadeUp").value = bs.fade_up ?? 0;
  const m = bs.map || { c: "0" };
  for (const [k, id] of [["c", "bsMapC"], ["l", "bsMapL"], ["r", "bsMapR"], ["ls", "bsMapLs"], ["rs", "bsMapRs"], ["lfe", "bsMapLfe"]]) $(id).value = m[k] ?? "";
  setDmx(p.downmix);
  bsCsvPath = ""; bsCsvPoints = [];
  if (bs.seamless === "csv" && bs.csv_path) {
    try {
      const text = await invoke("read_log", { path: bs.csv_path });
      bsCsvPoints = text.split(/\r?\n/).map((l) => l.split(",")[0].trim()).filter(Boolean);
      bsCsvPath = bs.csv_path;
    } catch (e) { /* the file moved: the note says no file is chosen */ }
  }
  syncBitstream();
  if (p.pcm_format) $("pcmFormat").value = p.pcm_format;
  $("dialnormOn").checked = !!p.dialnorm_on;
  $("dialnorm").disabled = !$("dialnormOn").checked;
  if (Number.isFinite(p.dialnorm)) $("dialnorm").value = p.dialnorm;
  syncDialnorm();
  renderBitrate("preset");
  if (p.bitrate != null && !$("bitrate").disabled) $("bitrate").value = p.bitrate;
  if (p.outname) { $("outname").value = p.outname; nameAuto = false; }
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
  buildDmxGrids();
  buildThemeGrid();
  initTips();
  toolPaths.ffmpeg = store.get("ffmpegPath") || "";
  toolPaths.mas = store.get("masPath") || "";
  await pushToolPaths();
  cat = await invoke("catalog");
  // Rust serialises the enum variant (Lfe); the rest of the app uses the short ID (LFE).
  cat.layouts = cat.layouts.map((l) => ({ ...l, channels: l.channels.map((c) => (c === "Lfe" ? "LFE" : c === "Lfe2" ? "LFE2" : c)) }));

  const eng = $("engine");
  eng.classList.add(cat.has_ffmpeg ? "ok" : "bad");
  eng.lastElementChild.textContent = cat.has_ffmpeg ? "FFmpeg engine ready" : "FFmpeg not found";

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
  $("pcmFormat").onchange = () => update();
  $("coreOn").onchange = () => { $("bitrate").disabled = coreOff(); syncX96(); syncChips(); update(); };
  $("x96Core").onchange = () => update();
  $("bsSeamless").onchange = syncBitstream;
  for (const id of ["dmxEmbed", "dmxMatrix", "dmxLtRt"]) $(id).onchange = syncDownmix;
  $("dmx51Grid").oninput = syncDownmix;
  $("mainTabs").onclick = (e) => { const b = e.target.closest("button"); if (b) showTab(b.dataset.tab); };
  addInfos();
  $("bsCsv").onclick = chooseBranchCsv;
  $("outname").oninput = () => { nameAuto = !$("outname").value.trim(); update(); };
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
  initSettings();
  $("presetSave").onclick = async () => {
    const f = await dialog.save({ defaultPath: ($("outname").value || "preset") + ".swpreset", filters: PRESET_FILTER });
    if (!f) return;
    try { await invoke("write_text", { path: f, text: JSON.stringify(presetState(), null, 2) }); log(`Saved preset ${f}`, "ok"); }
    catch (e) { log(String(e), "bad"); }
  };
  $("presetLoad").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: PRESET_FILTER });
    if (!f) return;
    try { await applyPreset(JSON.parse(await invoke("read_log", { path: f }))); log(`Loaded preset ${f}`, "ok"); setMode("encode"); }
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
      if (!$("atmos").hidden) setAtmosFile(p.paths[0]);
      else if (!$("verify").hidden) setVerifyFile(p.paths[0]);
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
  if (mode !== "settings") lastMode = mode;
  document.body.classList.toggle("tools-mode", mode !== "encode");
  $("tools").hidden = mode !== "tools";
  $("verify").hidden = mode !== "verify";
  $("atmos").hidden = mode !== "atmos";
  $("settingsPage").hidden = mode !== "settings";
  for (const [id, m] of [["modeEncode", "encode"], ["modeTools", "tools"], ["modeVerify", "verify"], ["modeAtmos", "atmos"]]) $(id).setAttribute("aria-pressed", String(mode === m));
  $("openSettings").setAttribute("aria-pressed", String(mode === "settings"));
}

function initTools() {
  $("modeEncode").onclick = () => setMode("encode");
  $("modeTools").onclick = () => setMode("tools");
  $("modeVerify").onclick = () => setMode("verify");
  $("modeAtmos").onclick = () => setMode("atmos");
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

const VERIFY_FILTERS = [{ name: "Encoded audio streams and files", extensions: ["ac3", "eac3", "ec3", "dts", "dtshd", "cpt", "thd", "mlp", "truehd", "wav", "aif", "aiff", "caf", "flac", "oga", "ogg", "m4a", "tta", "dsf", "dff"] }, { name: "All files", extensions: ["*"] }];
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


/* ---------- Atmos (Dolby Encoding Engine) ---------- */

/* ---------- settings page ---------- */


async function pushToolPaths() {
  await invoke("set_tool_paths", { ffmpeg: toolPaths.ffmpeg, masDir: toolPaths.mas });
}

function setStatus(el, ok, text) {
  el.className = "status " + (ok ? "ok" : "bad");
  el.lastElementChild.textContent = text;
}

async function refreshEngines() {
  const s = await invoke("tool_status");
  $("ffPath").value = toolPaths.ffmpeg;
  setStatus($("ffStatus"), !!s.ffmpeg, s.ffmpeg ? "found" : "not found");
  const ver = /version (\S+)/.exec(s.ffmpeg_version || "");
  $("ffVersion").textContent = s.ffmpeg ? `ffmpeg ${ver ? ver[1].split("-")[0] : ""}  ·  ${s.ffmpeg}` : "No ffmpeg on PATH, and none chosen.";
  $("ffVersion").title = s.ffmpeg_version || "";
  const eng = $("engine");
  eng.classList.toggle("ok", !!s.ffmpeg);
  eng.classList.toggle("bad", !s.ffmpeg);
  eng.lastElementChild.textContent = s.ffmpeg ? "FFmpeg engine ready" : "FFmpeg not found";

  $("masPath").value = toolPaths.mas;
  const have = s.mas_files.some(([, ok]) => ok);
  setStatus($("masStatus"), have, have ? "found" : "not found");
  $("masFiles").innerHTML = (s.mas_dir ? [[s.mas_dir, null]] : []).concat(s.mas_files).map(([n, ok]) => ok === null ? `<li class="yes"><i></i><span></span></li>` : `<li class="${ok ? "yes" : ""}"><i></i>${n}</li>`).join("") || `<li><i></i>No suite folder found</li>`;
  if (s.mas_dir) $("masFiles").firstChild.lastChild.textContent = s.mas_dir;

  const dee = store.get("dee") || "";
  $("deePath").value = dee;
  const found = await invoke("atmos_locate", { configured: dee || null });
  setStatus($("deeStatus"), !!found, found ? "found" : "not found");
  $("deePath").placeholder = found && !dee ? `Automatic: ${found}` : "Not set: Atmos needs dee.exe";
  $("deePath").title = found || "";
}

function showSettingsTab(name) {
  for (const b of $("sTabs").querySelectorAll("button[data-s]")) b.setAttribute("aria-pressed", String(b.dataset.s === name));
  for (const pg of document.querySelectorAll(".spage")) pg.hidden = pg.dataset.s !== name;
  if (name === "engines") refreshEngines();
}

function initSettings() {
  for (const b of $("sTabs").querySelectorAll("button[data-s]")) b.onclick = () => showSettingsTab(b.dataset.s);
  $("sBack").onclick = () => setMode(lastMode);
  $("openSettings").onclick = () => setMode(document.body.classList.contains("tools-mode") && !$("settingsPage").hidden ? lastMode : "settings");
  $("ffBrowse").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: [{ name: "ffmpeg", extensions: ["exe"] }, { name: "All files", extensions: ["*"] }] });
    if (f) { toolPaths.ffmpeg = f; store.set("ffmpegPath", f); await pushToolPaths(); await refreshEngines(); }
  };
  $("ffReset").onclick = async () => { toolPaths.ffmpeg = ""; store.set("ffmpegPath", ""); await pushToolPaths(); await refreshEngines(); };
  $("masBrowse").onclick = async () => {
    const d = await dialog.open({ directory: true });
    if (d) { toolPaths.mas = d; store.set("masPath", d); await pushToolPaths(); await refreshEngines(); }
  };
  $("masReset").onclick = async () => { toolPaths.mas = ""; store.set("masPath", ""); await pushToolPaths(); await refreshEngines(); };
  $("deeBrowse").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: [{ name: "Dolby Encoding Engine", extensions: ["exe"] }] });
    if (f) { store.set("dee", f); await refreshEngine(); await refreshEngines(); }
  };
  $("deeReset").onclick = async () => { store.set("dee", ""); await refreshEngine(); await refreshEngines(); };
}

const atm = { path: null, kind: "", dee: "", format: "truehd", running: false };

function aLog(text, cls) {
  const el = $("aLog");
  el.textContent = text;
  el.className = "selectable" + (cls ? " " + cls : "");
}

async function refreshEngine() {
  const found = await invoke("atmos_locate", { configured: store.get("dee") || null });
  atm.dee = found || "";
  const el = $("aEngine");
  el.className = "status " + (found ? "ok" : "bad");
  el.lastChild.textContent = found ? `Dolby Encoding Engine: ${found}` : "Dolby Encoding Engine: not found";
}

function atmosFormat(f) {
  atm.format = f;
  $("aFmtThd").setAttribute("aria-pressed", String(f === "truehd"));
  $("aFmtDdp").setAttribute("aria-pressed", String(f === "ddp"));
  $("aThdOpts").hidden = f !== "truehd";
  $("aClWrap").hidden = f !== "truehd";
  $("aBrWrap").hidden = f !== "ddp";
}

async function setAtmosFile(path) {
  atm.path = path;
  $("aPath").value = path;
  $("aVerdict").hidden = true;
  $("aBarWrap").hidden = true;
  $("aToVerify").hidden = true;
  $("aInfo").hidden = true;
  aLog("Reading the master…");
  try {
    const r = await invoke("atmos_inspect", { path, deePath: atm.dee || null });
    atm.kind = r.kind;
    $("aInfo").textContent = `${r.kind}\n${r.text}`;
    $("aInfo").hidden = false;
    const bed = r.kind.startsWith("channel bed");
    $("aFmtThd").disabled = bed;
    if (bed) atmosFormat("ddp");
    aLog("Ready.");
  } catch (e) {
    atm.kind = "";
    aLog(String(e), "bad");
  }
}

function initAtmos() {
  atmosFormat("truehd");
  refreshEngine();
  $("aFmtThd").onclick = () => { if (!$("aFmtThd").disabled) atmosFormat("truehd"); };
  $("aFmtDdp").onclick = () => atmosFormat("ddp");
  $("aLocate").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: [{ name: "Dolby Encoding Engine", extensions: ["exe"] }] });
    if (f) { store.set("dee", f); await refreshEngine(); }
  };
  $("aOpen").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: [{ name: "Atmos master or channel bed", extensions: ["atmos", "wav", "bw64", "rf64", "mxf"] }, { name: "All files", extensions: ["*"] }] });
    if (f) setAtmosFile(f);
  };
  $("aRun").onclick = async () => {
    if (atm.running) return;
    if (!atm.path) { aLog("Open a master first.", "bad"); return; }
    if (!atm.dee) { aLog("The Dolby Encoding Engine was not found. Use Locate dee.exe… first.", "bad"); return; }
    const ext = atm.format === "truehd" ? "thd" : "ec3";
    const stem = atm.path.replace(/\.[^.\\/]*$/, "");
    const out = await dialog.save({ defaultPath: `${stem}.${ext}`, filters: [{ name: atm.format === "truehd" ? "TrueHD Atmos" : "Dolby Digital Plus Atmos", extensions: [ext] }] });
    if (!out) return;
    const num = (id) => Number($(id).value) || 0;
    const req = {
      input: atm.path, output: out, dee: atm.dee, format: atm.format,
      clusters: num("aClusters"), drc: $("aDrc").value, legacy: $("aLegacy").checked,
      p8: $("aP8").checked, p6: $("aP6").checked, p2: $("aP2").checked, p2Format: "stereo", p2_format: "stereo",
      optimize: $("aOpt").checked, bitrate: num("aBr"), dialnorm: num("aDialnorm") || null,
      frame_rate: $("aFps").value || null, start: $("aStart").value || null, end: $("aEnd").value || null, metering: $("aMeter").value || null,
    };
    $("aVerdict").hidden = true;
    $("aToVerify").hidden = true;
    try { await invoke("atmos_start", { req }); } catch (e) { aLog(String(e), "bad"); return; }
    atm.running = true;
    $("aRun").disabled = true;
    $("aCancel").hidden = false;
    $("aBarWrap").hidden = false;
    $("aBar").className = "";
    const poll = setInterval(async () => {
      const st = await invoke("atmos_status");
      $("aBar").style.width = `${Math.round(st.progress * 100)}%`;
      aLog(st.lines.slice(-12).join("\n") || "Working…");
      if (st.running) return;
      clearInterval(poll);
      atm.running = false;
      $("aRun").disabled = false;
      $("aCancel").hidden = true;
      if (st.error) {
        $("aBar").className = "fail";
        aLog(st.error, "bad");
        return;
      }
      $("aBar").className = "done";
      const d = st.done;
      const v = d.verify && d.verify.report;
      const atmosLine = v && v.summary.find(([k]) => k === "Dolby Atmos");
      const vd = $("aVerdict");
      vd.hidden = false;
      vd.className = "verdict " + (v && v.passed ? "pass" : "fail");
      vd.innerHTML = `${v && v.passed ? "Done" : "Written, but the check found problems"}<small>${esc(d.output)} (${d.seconds.toFixed(1)} s)${atmosLine ? ` - Dolby Atmos: ${esc(atmosLine[1])}` : ""}</small>`;
      aLog(`Wrote ${d.output}`);
      atm.result = d.output;
      $("aToVerify").hidden = false;
    }, 400);
  };
  $("aCancel").onclick = () => invoke("atmos_cancel");
  $("aToVerify").onclick = () => { if (atm.result) { setVerifyFile(atm.result); setMode("verify"); } };
}
initAtmos();
