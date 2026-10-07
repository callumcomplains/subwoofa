const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const $ = (id) => document.getElementById(id);

let cat;               // catalog from the Rust side
let inputs = {};       // channel short name -> path
let meta = {};         // channel short name -> probe result
let outDir = "";

// Angle in degrees clockwise from front-centre, plus ring (0 = ear level, 1 = height).
const POS = {
  L: [-30, 0], R: [30, 0], C: [0, 0], LFE: [0, 2], Ls: [-110, 0], Rs: [110, 0], Cs: [180, 0],
  Lss: [-90, 0], Rss: [90, 0], Lsr: [-140, 0], Rsr: [140, 0], Lw: [-60, 0], Rw: [60, 0],
  Lh: [-45, 1], Rh: [45, 1], Lhs: [-90, 1], Rhs: [90, 1], Ch: [0, 1], Oh: [0, 3],
  Lt: [-30, 0], Rt: [30, 0],
};

const layoutById = (id) => cat.layouts.find((l) => l.id === id);
const curLayout = () => layoutById($("layout").value);
const curDest = () => cat.destinations.find((d) => d.id === $("dest").value);
const curStream = () => curDest().streams.find((s) => s.id === $("stream").value);
const fill = (sel, items, keep) => {
  const old = keep ?? sel.value;
  sel.innerHTML = "";
  for (const [v, t] of items) sel.add(new Option(t, v));
  if (items.some(([v]) => String(v) === String(old))) sel.value = old;
};
const base = (p) => p.split(/[\\/]/).pop();
const fmtTime = (s) => {
  if (s == null) return "";
  const h = Math.floor(s / 3600), m = Math.floor((s % 3600) / 60), sec = s % 60;
  return `${String(h).padStart(2, "0")}:${String(m).padStart(2, "0")}:${sec.toFixed(3).padStart(6, "0")}`;
};

function log(msg, cls = "") {
  const el = $("log");
  el.className = cls;
  el.textContent = msg;
  el.scrollTop = el.scrollHeight;
}

function drawSpeakers() {
  const svg = $("speakers");
  const l = curLayout();
  let h = `<circle class="ring" r="45"/><circle class="ring" r="85"/>`;
  h += `<path class="head" d="M0,-9 L7,6 L0,2 L-7,6 Z"/>`;
  for (const ch of l.channels) {
    const name = ch;
    const [deg, ring] = POS[name] || [0, 0];
    let r = ring === 1 ? 45 : 85, x, y;
    if (ring === 2) { x = 0; y = -65; r = 0; }          // LFE: front, between the rings
    else if (ring === 3) { x = 0; y = 0; r = 0; }       // overhead: centre
    else { const a = (deg * Math.PI) / 180; x = r * Math.sin(a); y = -r * Math.cos(a); }
    const height = ring === 1 || ring === 3;
    h += `<circle class="spk ${height ? "height" : ""}" cx="${x}" cy="${y}" r="10"/>`;
    h += `<text class="lbl" x="${x}" y="${y + (ring === 3 ? 18 : 0)}" ${height ? "" : 'style="fill:#1a0d00;font-weight:700"'}>${name}</text>`;
  }
  svg.innerHTML = h;
}

function renderFiles() {
  const tb = $("files").tBodies[0];
  tb.innerHTML = "";
  for (const ch of curLayout().channels) {
    const tr = tb.insertRow();
    tr.dataset.ch = ch;
    const m = meta[ch] || {};
    tr.innerHTML = `<td class="ch">${ch}</td><td><input type="text" readonly placeholder="drop or double-click…" value="${inputs[ch] ? base(inputs[ch]) : ""}" title="${inputs[ch] || ""}"></td>
      <td class="meta">${m.channels ? (m.channels === 1 ? "M" : m.channels) : ""}</td><td class="meta">${m.bits || ""}</td><td class="meta">${m.sample_rate || ""}</td><td class="meta">${fmtTime(m.duration)}</td>`;
    tr.querySelector("input").addEventListener("dblclick", () => browseFor(ch));
    tr.addEventListener("contextmenu", (e) => { e.preventDefault(); delete inputs[ch]; delete meta[ch]; renderFiles(); update(); });
  }
  const ok = checkMeta();
  for (const tr of tb.rows) tr.classList.toggle("err", ok.bad.includes(tr.dataset.ch));
}

function checkMeta() {
  const bad = [];
  const rate = Number($("rate").value);
  for (const [ch, m] of Object.entries(meta)) if (m.sample_rate && m.sample_rate !== rate) bad.push(ch);
  return { bad };
}

async function setFile(ch, path) {
  inputs[ch] = path;
  try { meta[ch] = await invoke("probe", { path }); } catch (e) { meta[ch] = {}; }
  renderFiles();
  update();
}

async function browseFor(ch) {
  const p = await dialog.open({ multiple: false, filters: [{ name: "Audio", extensions: ["wav", "aif", "aiff", "flac"] }] });
  if (!p) return;
  if (ch === curLayout().channels[0]) await autoImport(p); else await setFile(ch, p);
}

async function autoImport(path) {
  const found = await invoke("auto_import", { first: path, layout: $("layout").value });
  const entries = Object.entries(found);
  if (!entries.length) { log("No channel ID found in that file name; assign files by row instead.", "bad"); return; }
  await Promise.all(entries.map(([ch, p]) => setFile(ch, p)));
  log(`Auto-import matched ${entries.length}/${curLayout().channels.length} channels.`);
}

async function refreshOptions() {
  const o = await invoke("options", { dest: $("dest").value, stream: $("stream").value, layout: $("layout").value, sampleRate: Number($("rate").value) });
  return o;
}

async function cascade(from) {
  if (from === "dest") {
    fill($("stream"), curDest().streams.map((s) => [s.id, s.name]));
  }
  if (from === "dest" || from === "stream") {
    fill($("rate"), curStream().sample_rates.map((r) => [r, `${r / 1000} kHz`]), $("rate").value || 48000);
  }
  let o = await refreshOptions();
  // layouts valid for this combination; keep the choice when possible
  const old = $("layout").value;
  const valid = cat.layouts.filter((l) => o.layouts.includes(l.id));
  fill($("layout"), valid.map((l) => [l.id, l.name]), valid.some((l) => l.id === old) ? old : "5.1");
  if ($("layout").value !== old) o = await refreshOptions();
  const br = $("bitrate");
  if (o.bitrate_min == null) {
    br.disabled = true; br.value = ""; $("brange").textContent = "lossless / variable";
  } else {
    br.disabled = false; br.min = o.bitrate_min; br.max = o.bitrate_max;
    const cap = { ac3: 640, eac3: 1024 }[$("stream").value] ?? o.bitrate_max;
    if (!br.value || from !== "bitrate") br.value = Math.min(o.bitrate_max, cap);
    $("brange").textContent = `${o.bitrate_min}–${o.bitrate_max}`;
  }
  $("ext").textContent = "." + o.extension;
  drawSpeakers();
  renderFiles();
  update();
}

function update() {
  const need = curLayout().channels;
  const ready = need.every((c) => inputs[c]) && outDir && $("outname").value.trim() && !checkMeta().bad.length && cat.has_ffmpeg;
  $("go").disabled = !ready;
}

async function encode() {
  const entire = $("entire").checked;
  $("go").disabled = true;
  log("Encoding…");
  try {
    const dn = Number($("dialnorm").value);
    const r = await invoke("encode", { req: {
      dest: $("dest").value, stream: $("stream").value, layout: $("layout").value,
      sample_rate: Number($("rate").value),
      bitrate: $("bitrate").disabled ? null : Number($("bitrate").value),
      dialnorm: dn, start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
      inputs, output: `${outDir}\\${$("outname").value.trim()}${$("ext").textContent}`,
    }});
    log(r, "ok");
  } catch (e) { log(String(e), "bad"); }
  update();
}

async function init() {
  cat = await invoke("catalog");
  // Rust serialises the enum variant (Lfe); the rest of the app uses the short ID (LFE).
  cat.layouts = cat.layouts.map((l) => ({ ...l, channels: l.channels.map((c) => (c === "Lfe" ? "LFE" : c)) }));
  $("engine").textContent = cat.has_ffmpeg ? "FFmpeg engine ready" : "FFmpeg not found on PATH";
  $("engine").classList.add(cat.has_ffmpeg ? "ok" : "bad");
  fill($("dest"), cat.destinations.map((d) => [d.id, d.name]), "generic");
  fill($("dialnorm"), Array.from({ length: 31 }, (_, i) => [31 - i, `-${31 - i} dBFS${i === 0 ? " (no attenuation)" : ""}`]), 31);
  await cascade("dest");

  $("dest").onchange = () => cascade("dest");
  $("stream").onchange = () => cascade("stream");
  $("layout").onchange = () => { inputs = {}; meta = {}; cascade("layout"); };
  $("rate").onchange = () => cascade("rate");
  $("bitrate").onchange = () => cascade("bitrate");
  $("outname").oninput = update;
  $("entire").onchange = () => { $("start").disabled = $("duration").disabled = $("entire").checked; };
  $("clear").onclick = () => { inputs = {}; meta = {}; renderFiles(); update(); };
  $("browse").onclick = async () => { const d = await dialog.open({ directory: true }); if (d) { outDir = d; $("outdir").value = d; update(); } };
  $("go").onclick = encode;

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
