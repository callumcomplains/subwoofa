// The channel layout picker, the input files table and the format cascade (destination, stream, layout, rate, bit rate).

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
  const file = ["wav", "aiff", "caf", "flac", "oggflac", "alac", "tta", "wavpack", "ape", "als", "dcp-audio", "dsf", "dff"].includes(st);
  $("pcmWrap").hidden = !file;
  document.querySelector(".f.dialnorm").hidden = file; // no dialogue level in a plain audio file
  if (!file) return;
  const dsd = st === "dsf" || st === "dff";
  const dcp = st === "dcp-audio";
  $("pcmWrap").querySelector("label span").textContent = dcp ? "Frame rate" : "Sample format";
  const flac = ["flac", "oggflac", "alac", "tta", "wavpack", "ape", "als"].includes(st);
  // DSD offers only the DSD rates; the integer-only formats only 16 and 24 bit; the rest no DSD
  const allowed = (o) => dcp ? o.hasAttribute("data-dcp") : o.hasAttribute("data-dcp") ? false : dsd ? o.value.startsWith("dsd") : o.value.startsWith("dsd") ? false : flac ? ["s16", "s24"].includes(o.value) : true;
  for (const o of $("pcmFormat").options) { o.disabled = !allowed(o); o.hidden = o.disabled; }
  if ($("pcmFormat").selectedOptions[0].disabled) $("pcmFormat").value = dcp ? "24" : dsd ? "dsd64" : "s24";
  setTip("#pcmFormat", dcp
    ? "Cinema sound is always 24-bit PCM at 48 or 96 kHz. Choose the frame rate of the picture it goes with: each frame of sound is one picture frame long. SMPTE is the current standard, MXF Interop the older one some servers still want."
    : dsd
    ? "One-bit DSD made with a fifth-order sigma-delta modulator at half scale (-6 dB), the filters keeping the band up to about 20 kHz (40 kHz from 88.2 kHz sources). Audio only: no disc image."
    : st === "tta"
    ? "TTA stores 16 or 24 bit integers."
    : st === "als"
    ? "MPEG-4 ALS stores 16 or 24 bit integers. Name the file .mp4 instead of .als to get an MP4 that FFmpeg and most players open."
    : st === "ape"
    ? "Monkey's Audio stores 16 or 24 bit integers, mono or stereo only."
    : st === "wavpack"
    ? "WavPack stores 16 or 24 bit integers, any number of channels (split into pairs, as the format does)."
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
    fill($("stream"), list.map((s) => [s.id, !s.available ? `${s.name} (planned)` : !cat.has_ffmpeg && !s.native ? `${s.name} (needs FFmpeg)` : s.name]), $("stream").value);
    if (!cat.has_ffmpeg) for (const o of $("stream").options) if (/needs FFmpeg/.test(o.textContent)) o.disabled = true;
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
