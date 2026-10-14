// Dolby Atmos through the Dolby Encoding Engine.

const atm = { path: null, kind: "", dee: "", format: "truehd", running: false };

function aLog(text, cls) {
  const el = $("aLog");
  el.textContent = text;
  el.className = "selectable" + (cls ? " " + cls : "");
}

async function refreshEngine() {
  const found = await invoke("atmos_locate", { configured: store.get("dee") || null });
  atm.dee = found || "";
  invoke("set_dee_path", { path: atm.dee });
  if (typeof syncDolbyUi === "function" && $("dnDolbyWrap")) syncDolbyUi();
  $("atmos").classList.toggle("locked", !found);
  $("atmosLock").hidden = !!found;
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
  $("aTrimWrap").hidden = f !== "ddp";
  $("aHtTrimWrap").hidden = f !== "ddp";
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
      dialogue: $("aDialogue").value === "" ? null : $("aDialogue").value === "on",
      speech_threshold: $("aSpeech").value === "" ? null : num("aSpeech"),
      surround_trim: $("aSurTrim").value, height_trim: $("aHtTrim").value,
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
