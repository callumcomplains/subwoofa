const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const $ = (id) => document.getElementById(id);

let cat;                 // catalog from the Rust side
let inputs = {};         // channel -> path
let meta = {};           // channel -> probe result
let outDir = "";
let opts = null;         // last options() answer
let unlocked = false;

const COMMON_RATES = [8000, 11025, 16000, 22050, 32000, 44100, 48000, 88200, 96000, 176400, 192000, 352800, 384000];
const DEFAULT_BITRATE = { ac3: 640, eac3: 1024, dts: 1509 };

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
  "1.1": "Mono + LFE (DVD only)", "1.0": "Mono",
};

// Angle in degrees clockwise from front-centre, plus ring (0 ear level, 1 height, 2 LFE, 3 overhead).
const POS = {
  L: [-30, 0], R: [30, 0], C: [0, 0], LFE: [0, 2], Ls: [-110, 0], Rs: [110, 0], Cs: [180, 0],
  Lss: [-90, 0], Rss: [90, 0], Lsr: [-140, 0], Rsr: [140, 0], Lw: [-60, 0], Rw: [60, 0],
  Lh: [-45, 1], Rh: [45, 1], Lhs: [-90, 1], Rhs: [90, 1], Ch: [0, 1], Oh: [0, 3], Lt: [-30, 0], Rt: [30, 0],
};

const family = (l) => `${l.channels.filter((c) => c !== "LFE").length}.${l.channels.includes("LFE") ? 1 : 0}`;
const layoutById = (id) => cat.layouts.find((l) => l.id === id);
const curLayoutId = () => $("variants").dataset.value;
const curLayout = () => layoutById(curLayoutId());
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
  inputs = {}; meta = {};
  cascade("layout", id);
}

function drawSpeakers() {
  const l = curLayout();
  let h = `<circle class="ring" r="44"/><circle class="ring" r="82"/><path class="you" d="M0,-8 L6,6 L0,2.5 L-6,6 Z"/>`;
  for (const ch of l.channels) {
    const [deg, ring] = POS[ch] || [0, 0];
    let x, y;
    if (ring === 2) { x = 0; y = -21; }                      // LFE: inside the inner ring, clear of the speakers
    else if (ring === 3) { x = 0; y = 0; }                   // overhead: above the listener
    else { const r = ring === 1 ? 44 : 82, a = (deg * Math.PI) / 180; x = r * Math.sin(a); y = -r * Math.cos(a); }
    const height = ring === 1 || ring === 3;
    h += `<circle class="spk${height ? " height" : ""}" cx="${x.toFixed(1)}" cy="${y.toFixed(1)}" r="9.5"/>`;
    h += `<text class="lbl${height ? " height" : ""}" x="${x.toFixed(1)}" y="${y.toFixed(1)}">${ch}</text>`;
  }
  $("speakers").innerHTML = h;
}

/* ---------- inputs table ---------- */

function renderFiles() {
  const tb = $("files").tBodies[0];
  tb.innerHTML = "";
  const bad = rateMismatch();
  for (const ch of curLayout().channels) {
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
  if (ch === curLayout().channels[0]) await autoImport(p); else await setFile(ch, p);
}

async function autoImport(path) {
  const found = await invoke("auto_import", { first: path, layout: curLayoutId() });
  const entries = Object.entries(found);
  if (!entries.length) { log("No channel ID found in that file name. Assign files row by row instead.", "bad"); return; }
  await Promise.all(entries.map(([ch, p]) => setFile(ch, p)));
  log(`Auto-import matched ${entries.length} of ${curLayout().channels.length} channels.`);
}

/* ---------- format cascade ---------- */

async function loadOptions() {
  opts = await invoke("options", { dest: $("dest").value, stream: $("stream").value, layout: curLayoutId() || "5.1", sampleRate: curRate() || 48000, unlocked });
  return opts;
}

function renderRates(want) {
  const s = curStream();
  const rates = unlocked ? [...new Set([...s.sample_rates, ...COMMON_RATES])].sort((a, b) => a - b) : s.sample_rates;
  const items = rates.map((r) => [r, fmtRate(r)]);
  if (unlocked) items.push(["custom", "Custom…"]);
  const keep = rates.includes(want) || want === "custom" ? want : s.sample_rates.includes(48000) ? 48000 : s.sample_rates[0];
  fill($("rate"), items, keep);
  $("ratecustom").hidden = $("rate").value !== "custom";
}

function renderBitrate(from) {
  const br = $("bitrate"), o = opts;
  const lossless = o.bitrate_min == null;
  br.disabled = lossless && !unlocked;
  $("brange").textContent = lossless ? (unlocked ? "free entry" : "lossless / variable") : unlocked ? `spec ${o.bitrate_min}–${o.bitrate_max}` : `${o.bitrate_min}–${o.bitrate_max}`;
  if (lossless && !unlocked) br.value = "";
  else if (from === "dest" || from === "stream" || !br.value) {
    br.value = lossless ? "" : (DEFAULT_BITRATE[$("stream").value] ?? o.bitrate_max);
    if (!lossless) br.value = Math.min(o.bitrate_max, br.value);
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
  const need = curLayout().channels;
  const dnOk = !$("dialnorm").classList.contains("bad");
  const ready = need.every((c) => inputs[c]) && outDir && $("outname").value.trim() && !rateMismatch().length && bitrateOk() && dnOk && cat.has_ffmpeg && curRate() > 0;
  const available = curStream().available;
  $("go").disabled = !ready || !available;
  if (!available) log(`${curStream().name} has no encoder yet. It is on the roadmap; for now this format is listed so you can see what is coming.`);
  syncChips();
}

async function encode() {
  const entire = $("entire").checked;
  $("go").disabled = true;
  log("Encoding…");
  try {
    const r = await invoke("encode", { req: {
      dest: $("dest").value, stream: $("stream").value, layout: curLayoutId(),
      sample_rate: curRate(),
      bitrate: $("bitrate").disabled || !$("bitrate").value ? null : Number($("bitrate").value),
      dialnorm: dialnormValue(), unlocked,
      start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
      inputs, output: `${outDir}\\${$("outname").value.trim()}${$("ext").textContent}`,
    }});
    log(r, "ok");
  } catch (e) { log(String(e), "bad"); }
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
  cat.layouts = cat.layouts.map((l) => ({ ...l, channels: l.channels.map((c) => (c === "Lfe" ? "LFE" : c)) }));

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
  $("outname").oninput = update;
  $("entire").onchange = () => { $("start").disabled = $("duration").disabled = $("entire").checked; };
  $("clear").onclick = () => { inputs = {}; meta = {}; renderFiles(); update(); };
  $("browse").onclick = async () => { const d = await dialog.open({ directory: true }); if (d) { outDir = d; $("outdir").value = d; update(); } };
  $("go").onclick = encode;
  $("openSettings").onclick = () => $("settings").showModal();
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
      if (r) await setFile(r.dataset.ch, p.paths[0]); else await autoImport(p.paths[0]);
    }
  });
}
init().catch((e) => log(String(e), "bad"));
