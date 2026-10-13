// StreamTools: info, bit rate chart, trim, split, join, relabel and silence.

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
