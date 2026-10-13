// The listening preview: encodes a short excerpt with the current settings, then plays the source, the encode and
// the difference between them in sync, so switching between them is instant and the only thing that changes is the format.

const PREVIEW_SECONDS = 20;
const DIFFERENCE_BOOST = 4;   // the difference is quiet by nature; +12 dB makes it audible
const pv = { ctx: null, buffers: {}, nodes: {}, gains: {}, which: "encoded", playing: false, offset: 0, began: 0, length: 0, busy: false, raf: 0 };

/** The preview needs the same things an encode does, except somewhere to put the file. */
function previewReady() {
  return inLayout().channels.every((c) => inputs[c]) && !rateMismatch().length && bitrateOk() && !$("dialnorm").classList.contains("bad")
    && (cat.has_ffmpeg || curStream().native) && curRate() > 0 && curStream().available && (!srcLayoutId || curStream().native);
}

function pvInfo(text, cls = "") { $("pvInfo").textContent = text; $("pvInfo").className = `readout ${cls}`; }

async function startPreview() {
  if (pv.busy) { invoke("preview_cancel"); return; }
  stopPlayback();
  pv.busy = true;
  $("previewPanel").hidden = false;
  $("previewBtn").textContent = "Cancel";
  $("pvPlayer").hidden = true;
  $("pvProgress").hidden = false; $("pvBar").style.width = "0%";
  pvInfo("Encoding a taste...");
  const unlisten = await window.__TAURI__.event.listen("preview", (e) => { $("pvBar").style.width = Math.round(e.payload * 100) + "%"; });
  try {
    const p = await invoke("make_preview", { spec: jobSpec(inputs, "preview"), seconds: PREVIEW_SECONDS });
    pv.ctx = pv.ctx || new AudioContext({ sampleRate: p.rate });
    if (pv.ctx.sampleRate !== p.rate) { await pv.ctx.close(); pv.ctx = new AudioContext({ sampleRate: p.rate }); }
    for (const key of ["source", "encoded", "difference"]) {
      const bytes = await invoke("read_preview_file", { path: p[key] });
      pv.buffers[key] = await pv.ctx.decodeAudioData(bytes.slice ? bytes.slice(0) : bytes);
    }
    pv.length = pv.buffers.source.duration; pv.offset = 0;
    $("pvPlayer").hidden = false;
    $("pvSeek").value = 0;
    paintPosition();
    const closeness = p.snr_db == null ? "identical to the source" : `${p.snr_db.toFixed(1)} dB signal to noise`;
    pvInfo(`${fmtClock(p.start)}, ${Math.round(p.seconds)} s: ${Math.round(p.kbps)} kbps, ${closeness}.${p.note ? " " + p.note : ""}`);
    selectWhich(pv.which);
  } catch (e) {
    $("pvPlayer").hidden = true;
    pvInfo(String(e) === "canceled" ? "Canceled." : String(e), String(e) === "canceled" ? "" : "bad");
  } finally {
    unlisten();
    pv.busy = false;
    $("pvProgress").hidden = true;
    $("previewBtn").textContent = "Preview";
  }
}

function selectWhich(which) {
  pv.which = which;
  for (const b of $("pvSeg").children) b.setAttribute("aria-pressed", String(b.dataset.which === which));
  if (!pv.gains.source) return;
  const now = pv.ctx.currentTime;
  for (const key of Object.keys(pv.gains)) {
    pv.gains[key].gain.setTargetAtTime(key === which ? (key === "difference" ? DIFFERENCE_BOOST : 1) : 0, now, 0.006);
  }
}

function position() {
  return pv.playing ? (pv.offset + pv.ctx.currentTime - pv.began) % pv.length : pv.offset;
}

function startPlayback() {
  if (!pv.buffers.source) return;
  pv.ctx.resume();
  const loop = $("pvLoop").checked;
  const when = pv.ctx.currentTime + 0.03;
  for (const key of ["source", "encoded", "difference"]) {
    const node = pv.ctx.createBufferSource();
    node.buffer = pv.buffers[key]; node.loop = loop;
    const gain = pv.ctx.createGain();
    gain.gain.value = key === pv.which ? (key === "difference" ? DIFFERENCE_BOOST : 1) : 0;
    node.connect(gain).connect(pv.ctx.destination);
    node.start(when, pv.offset);
    node.onended = () => { if (pv.nodes[key] === node && !loop && pv.playing) { stopPlayback(); pv.offset = 0; paintPosition(); } };
    pv.nodes[key] = node; pv.gains[key] = gain;
  }
  pv.began = when; pv.playing = true;
  syncPlayButton();
  tick();
}

function stopPlayback() {
  if (pv.playing) pv.offset = position();
  pv.playing = false;
  for (const node of Object.values(pv.nodes)) { try { node.stop(); } catch { /* already stopped */ } }
  pv.nodes = {};
  cancelAnimationFrame(pv.raf);
  syncPlayButton();
}

function syncPlayButton() {
  $("pvPlay").setAttribute("aria-label", pv.playing ? "Pause" : "Play");
  $("pvPlay").classList.toggle("on", pv.playing);
  $("pvPlay").querySelector("use").setAttribute("href", pv.playing ? "#i-pause" : "#i-play");
}

function paintPosition() {
  const p = position();
  $("pvSeek").value = Math.round((p / pv.length) * 1000);
  $("pvTime").textContent = `${fmtClock(p)} / ${fmtClock(pv.length)}`;
}

function tick() {
  paintPosition();
  if (pv.playing) pv.raf = requestAnimationFrame(tick);
}

function initPreview() {
  $("previewBtn").onclick = startPreview;
  $("pvClose").onclick = () => { stopPlayback(); if (pv.busy) invoke("preview_cancel"); $("previewPanel").hidden = true; };
  $("pvPlay").onclick = () => (pv.playing ? stopPlayback() : startPlayback());
  $("pvSeg").onclick = (e) => { const b = e.target.closest("button"); if (b) selectWhich(b.dataset.which); };
  $("pvLoop").onchange = () => { for (const n of Object.values(pv.nodes)) n.loop = $("pvLoop").checked; };
  $("pvSeek").oninput = () => {
    const was = pv.playing;
    if (was) stopPlayback();
    pv.offset = ($("pvSeek").value / 1000) * pv.length;
    paintPosition();
    if (was) startPlayback();
  };
  document.addEventListener("keydown", (e) => {
    if ($("previewPanel").hidden || $("pvPlayer").hidden || e.target.closest?.("input:not([type=range]):not([type=checkbox]), select, textarea")) return;
    const which = { 1: "source", 2: "encoded", 3: "difference" }[e.key];
    if (which) selectWhich(which);
    else if (e.key === " " && !e.target.closest?.("button")) { e.preventDefault(); $("pvPlay").click(); }
  });
}
