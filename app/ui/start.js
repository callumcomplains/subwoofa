// Start-up: reads the catalogue, restores settings and wires the main page.

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

  fill($("vLayout"), cat.layouts.map((l) => [l.id, l.id]), "5.1");
  initBanner();
  const ver = await invoke("app_version");
  $("ver").textContent = ver;
  $("verSide").textContent = `Subwoofa ${ver}`;
  const eng = $("engine");
  eng.classList.add(cat.has_ffmpeg ? "ok" : "bad");
  eng.lastElementChild.textContent = cat.has_ffmpeg ? "FFmpeg engine ready" : "FFmpeg not found";

  fill($("dest"), cat.destinations.map((d) => [d.id, d.name]), "generic");
  $("dialnormOn").onchange = () => { $("dialnorm").disabled = !$("dialnormOn").checked; syncDialnorm(); };
  $("dialnorm").oninput = syncDialnorm;
  $("dialnormMeasure").onclick = measureLoudness;
  $("normOn").onchange = update;
  $("delay").oninput = update;
  $("normTarget").oninput = update;
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
  $("watchBtn").onclick = toggleWatch;
  $("clear").onclick = () => { inputs = {}; meta = {}; renderFiles(); update(); };
  $("srcLayout").onchange = () => { srcLayoutId = $("srcLayout").value; inputs = {}; meta = {}; renderFiles(); update(); };
  $("browse").onclick = async () => { const d = await dialog.open({ directory: true }); if (d) { outDir = d; $("outdir").value = d; update(); } };
  $("go").onclick = encode;
  initPreview();
  $("qClear").onclick = () => invoke("queue_clear");
  const conc = Number(store.get("conc")) || 2;
  $("optConc").value = conc;
  invoke("queue_set_concurrency", { n: conc });
  $("optConc").onchange = () => { const n = Math.max(1, Math.min(8, Number($("optConc").value) || 1)); $("optConc").value = n; store.set("conc", n); invoke("queue_set_concurrency", { n }); };
  speed = ["fast", "balanced", "best"].includes(store.get("speed")) ? store.get("speed") : "balanced";
  threads = Number(store.get("threads")) || 0;
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
