// The Dolby Encoding Engine's everyday jobs: Dolby's dialogue-aware meter for Measure, Dolby's own encoder for
// Dolby Digital and Digital Plus, and the Dolby tab of StreamTools (decode, transcode, trim and join, correct loudness).
// The engine is found by the Atmos tab's check (atm.dee); everything here shows only when it is there.

const dx = { files: [], running: false };

/** Whether the engine is available. */
const dolbyFound = () => !!atm.dee;

/** Shows or hides everything that needs the engine. */
function syncDolbyUi() {
  const found = dolbyFound();
  $("dnDolbyWrap").hidden = !found;
  $("dolbyEncRow").hidden = !found;
  document.querySelector(".op-tabs button[data-op='dolby']").hidden = !found;
  if (!found && !$("dolbyPanel").hidden) document.querySelector(".op-tabs button[data-op='trim']").click();
}

/** Measure with Dolby's meter; returns the numbers the Measure button shows. */
async function measureWithDolby(files, entire) {
  const m = await invoke("measure_loudness_dolby", {
    layout: inLayout().id, inputs: files,
    start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
    dialogue: true,
  });
  const dialnorm = m.integrated == null ? null : Math.min(31, Math.max(1, Math.round(-m.integrated)));
  const peak = m.true_peak.length ? Math.max(...m.true_peak) : -Infinity;
  const how = m.gating === "speech" ? `dialogue, ${Math.round(m.speech_percent || 0)}% of the programme` : "whole programme";
  return { integrated: m.integrated, range: m.range, peak, dialnorm, how };
}

/* ---------- the StreamTools tab ---------- */

const DX_OPS = {
  decode: { title: "Decode to WAV", add: "Add stream…", filter: [{ name: "AC-3 / E-AC-3", extensions: ["ac3", "eac3", "ec3"] }], out: [{ name: "WAV", extensions: ["wav"] }], suffix: "_decoded.wav", many: false, opts: ["dxDrcWrap"] },
  transcode: { title: "Transcode to Dolby Digital Plus", add: "Add stream…", filter: [{ name: "AC-3 / E-AC-3", extensions: ["ac3", "eac3", "ec3"] }], out: [{ name: "E-AC-3", extensions: ["ec3"] }], suffix: "_ddp.ec3", many: false, opts: ["dxRateWrap", "dxPresetWrap"] },
  edit: { title: "Trim and join E-AC-3", add: "Add stream…", filter: [{ name: "E-AC-3", extensions: ["ec3", "eac3"] }], out: [{ name: "E-AC-3", extensions: ["ec3"] }], suffix: "_edit.ec3", many: true, opts: ["dxSpanWrap"] },
  correct: { title: "Correct loudness of a WAV", add: "Add WAV…", filter: [{ name: "WAV", extensions: ["wav"] }], out: [{ name: "WAV", extensions: ["wav"] }], suffix: "_corrected.wav", many: false, opts: ["dxPresetWrap2"] },
};

function dxRender() {
  const list = $("dxList");
  list.innerHTML = "";
  dx.files.forEach((p, i) => {
    const li = document.createElement("li");
    li.innerHTML = "<span></span>";
    li.firstChild.textContent = p;
    const b = document.createElement("button");
    b.className = "plain"; b.textContent = "Remove";
    b.onclick = () => { dx.files.splice(i, 1); dxRender(); };
    li.append(b);
    list.append(li);
  });
}

function dxSync() {
  const op = DX_OPS[$("dxOp").value];
  $("dxAdd").textContent = op.add;
  for (const id of ["dxDrcWrap", "dxRateWrap", "dxPresetWrap", "dxPresetWrap2", "dxSpanWrap"]) $(id).hidden = !op.opts.includes(id);
  if (!op.many && dx.files.length > 1) dx.files.length = 1;
  dxRender();
}

function dxJob(opName, output) {
  const [first] = dx.files;
  const stream = { kind: "stream", path: first, atmos: false };
  const task = {
    decode: () => ({ task: "decode", drc: $("dxDrc").value }),
    transcode: () => ({ task: "transcode", data_rate_kbps: $("dxRate").value ? Number($("dxRate").value) : null, correct: $("dxPreset").value || null }),
    edit: () => ({ task: "edit", start: $("dxStart").value.trim() || "0:0:0.0", end: $("dxEnd").value.trim() || "end_of_file", invert_trim: $("dxInvert").checked }),
    correct: () => ({ task: "correct", preset: $("dxPreset2").value }),
  }[opName]();
  const source = opName === "edit" ? { kind: "streams", paths: dx.files } : opName === "correct" ? { kind: "wav", path: first } : stream;
  return { source, output, task, start: null, end: null };
}

async function dxRun() {
  if (dx.running) { invoke("dolby_cancel"); return; }
  const opName = $("dxOp").value, op = DX_OPS[opName];
  if (!dx.files.length) { toolsLog("Add a file first.", "bad"); return; }
  const dot = dx.files[0].lastIndexOf(".");
  const out = await dialog.save({ defaultPath: dx.files[0].slice(0, dot) + op.suffix, filters: op.out });
  if (!out) return;
  dx.running = true;
  $("dxRun").textContent = "Cancel";
  $("dxBarWrap").hidden = false; $("dxBar").style.width = "0%";
  toolsLog(`${op.title}…`);
  const unlisten = await window.__TAURI__.event.listen("dolby", (e) => { $("dxBar").style.width = Math.round(e.payload * 100) + "%"; });
  try {
    const done = await invoke("dolby_run", { job: dxJob(opName, out) });
    const extra = done.measured && done.measured.integrated != null ? `\nLoudness of the result: ${done.measured.integrated.toFixed(1)} LUFS` : "";
    toolsLog(`Wrote ${done.output} in ${done.seconds.toFixed(1)} s${extra}${done.messages.length ? "\n" + done.messages.join("\n") : ""}`, "ok");
  } catch (e) {
    toolsLog(String(e) === "canceled" ? "Canceled." : String(e), String(e) === "canceled" ? "" : "bad");
  } finally {
    unlisten();
    dx.running = false;
    $("dxRun").textContent = "Run…";
    $("dxBarWrap").hidden = true;
  }
}

function initDolby() {
  $("dxOp").onchange = dxSync;
  $("dxAdd").onclick = async () => {
    const op = DX_OPS[$("dxOp").value];
    const f = await dialog.open({ multiple: op.many, filters: op.filter });
    if (!f) return;
    const picked = Array.isArray(f) ? f : [f];
    dx.files = op.many ? [...dx.files, ...picked] : picked.slice(0, 1);
    dxRender();
  };
  $("dxClear").onclick = () => { dx.files = []; dxRender(); };
  $("dxRun").onclick = dxRun;
  $("optDolbyEnc").checked = store.get("dolbyEnc") === "1";
  $("optDolbyEnc").onchange = () => { store.set("dolbyEnc", $("optDolbyEnc").checked ? "1" : "0"); };
  dxSync();
  syncDolbyUi();
}

/** Whether a job for this stream should go to Dolby's own encoder. */
function useDolbyEncoder() {
  return dolbyFound() && $("optDolbyEnc").checked && ["ac3", "eac3"].includes($("stream").value);
}
