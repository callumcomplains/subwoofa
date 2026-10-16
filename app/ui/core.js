// State of the page, small helpers and the log line.

const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const $ = (id) => document.getElementById(id);
// text made safe to put inside HTML, attribute values included
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

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
let speed = "balanced";   // lossless search effort: fast, balanced or best
let threads = 0;           // processor cores per encode; 0 = all

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
