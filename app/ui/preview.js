// The listening preview: encodes a short excerpt with the current settings, then plays the source, the encode and
// the difference between them in sync, so switching between them is instant and the only thing that changes is the format.

const DIFFERENCE_BOOST = 4;   // the difference is quiet by nature; +12 dB makes it audible
const pv = { ctx: null, buffers: {}, nodes: {}, gains: {}, which: "encoded", playing: false, offset: 0, began: 0, length: 0, busy: false, raf: 0 };

/** The preview needs the same things an encode does, except somewhere to put the file. */
function previewReady() {
  return inLayout().channels.every((c) => inputs[c]) && !rateMismatch().length && bitrateOk() && !$("dialnorm").classList.contains("bad")
    && (cat.has_ffmpeg || curStream().native) && curRate() > 0 && curStream().available && (!srcLayoutId || curStream().native);
}

function pvInfo(text, cls = "") { $("pvInfo").textContent = text; $("pvInfo").title = text; $("pvInfo").className = `readout ${cls}`; }

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
    // the excerpt: where it starts (the middle of the programme when empty) and how long it is
    const spec = jobSpec(inputs, "preview");
    spec.start = $("pvFrom").value === "" ? null : Math.max(0, Number($("pvFrom").value));
    spec.duration = null;
    const p = await invoke("make_preview", { spec, seconds: Number($("pvLen").value) || 20 });
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
    setBlind($("pvBlind").checked);
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

/** Blind mode: the buttons read A and B, which are the source and the encode in a random order, and the difference is out of reach. */
function setBlind(on) {
  const buttons = [...$("pvSeg").children];
  if (on) {
    const flip = Math.random() < 0.5;
    pv.blind = { A: flip ? "source" : "encoded", B: flip ? "encoded" : "source" };
    buttons[0].textContent = "A"; buttons[0].dataset.which = pv.blind.A;
    buttons[1].textContent = "B"; buttons[1].dataset.which = pv.blind.B;
    buttons[2].hidden = true;
    $("pvQuiz").hidden = false; $("pvAnswer").textContent = "";
    for (const id of ["pvGuessA", "pvGuessB"]) $(id).disabled = false;
    selectWhich(pv.blind.A);
  } else {
    pv.blind = null;
    buttons[0].textContent = "Source"; buttons[0].dataset.which = "source";
    buttons[1].textContent = "Encoded"; buttons[1].dataset.which = "encoded";
    buttons[2].hidden = false;
    $("pvQuiz").hidden = true;
    selectWhich("encoded");
  }
}

function guess(letter) {
  const right = pv.blind[letter] === "encoded";
  $("pvAnswer").textContent = right
    ? `Right: ${letter} is the encode. You can hear the format.`
    : `Not this time: ${letter} was the source, and the encode was ${letter === "A" ? "B" : "A"}. It passes for the original.`;
  for (const id of ["pvGuessA", "pvGuessB"]) $(id).disabled = true;
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
  $("pvBlind").onchange = () => { if (pv.buffers.source) setBlind($("pvBlind").checked); };
  $("pvGuessA").onclick = () => guess("A");
  $("pvGuessB").onclick = () => guess("B");
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
    const which = pv.blind ? { 1: pv.blind.A, 2: pv.blind.B }[e.key] : { 1: "source", 2: "encoded", 3: "difference" }[e.key];
    if (which) selectWhich(which);
    else if (e.key === " " && !e.target.closest?.("button")) { e.preventDefault(); $("pvPlay").click(); }
  });
}
