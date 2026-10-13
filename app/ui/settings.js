// The missing-tools banner and the Settings page (engines, performance, appearance).

/* ---------- missing tools ---------- */

const toEngines = () => { setMode("settings"); showSettingsTab("engines"); };

function initBanner() {
  const show = !cat.has_ffmpeg && sessionStorage.getItem("ffBannerOff") !== "1";
  $("toolBanner").hidden = !show;
  $("toolBannerText").textContent = "FFmpeg was not found. Subwoofa's own encoders still work. Anything that needs FFmpeg (reading files other than WAV and AIFF, a few formats) stays off until you point Subwoofa at it.";
  $("toolBannerSetup").onclick = toEngines;
  $("toolBannerHow").onclick = () => invoke("open_docs", { page: "ENGINES.md" });
  $("toolBannerClose").onclick = () => { try { sessionStorage.setItem("ffBannerOff", "1"); } catch { /* fine */ } $("toolBanner").hidden = true; };
  $("atmosLockSetup").onclick = toEngines;
  $("atmosLockHow").onclick = () => invoke("open_docs", { page: "ATMOS.md" });
}

/* ---------- settings page ---------- */


async function pushToolPaths() {
  await invoke("set_tool_paths", { ffmpeg: toolPaths.ffmpeg, masDir: toolPaths.mas });
  if (cat) {
    const had = cat.has_ffmpeg;
    cat = await invoke("catalog");
    if (cat.has_ffmpeg !== had) { cascade && (await cascade("dest")); initBanner(); }
  }
}

function setStatus(el, ok, text) {
  el.className = "status " + (ok ? "ok" : "bad");
  el.lastElementChild.textContent = text;
}

async function refreshEngines() {
  const s = await invoke("tool_status");
  $("ffPath").value = toolPaths.ffmpeg;
  setStatus($("ffStatus"), !!s.ffmpeg, s.ffmpeg ? "found" : "not found");
  const ver = /version (\S+)/.exec(s.ffmpeg_version || "");
  $("ffVersion").textContent = s.ffmpeg ? `ffmpeg ${ver ? ver[1].split("-")[0] : ""}  ·  ${s.ffmpeg}` : "No ffmpeg on PATH, and none chosen.";
  $("ffVersion").title = s.ffmpeg_version || "";
  const eng = $("engine");
  eng.classList.toggle("ok", !!s.ffmpeg);
  eng.classList.toggle("bad", !s.ffmpeg);
  eng.lastElementChild.textContent = s.ffmpeg ? "FFmpeg engine ready" : "FFmpeg not found";

  $("masPath").value = toolPaths.mas;
  const have = s.mas_files.some(([, ok]) => ok);
  setStatus($("masStatus"), have, have ? "found" : "not found");
  $("masFiles").innerHTML = (s.mas_dir ? [[s.mas_dir, null]] : []).concat(s.mas_files).map(([n, ok]) => ok === null ? `<li class="yes"><i></i><span></span></li>` : `<li class="${ok ? "yes" : ""}"><i></i>${n}</li>`).join("") || `<li><i></i>No suite folder found</li>`;
  if (s.mas_dir) $("masFiles").firstChild.lastChild.textContent = s.mas_dir;

  const dee = store.get("dee") || "";
  $("deePath").value = dee;
  const found = await invoke("atmos_locate", { configured: dee || null });
  setStatus($("deeStatus"), !!found, found ? "found" : "not found");
  $("deePath").placeholder = found && !dee ? `Automatic: ${found}` : "Not set: Atmos needs dee.exe";
  $("deePath").title = found || "";
}

function showSettingsTab(name) {
  for (const b of $("sTabs").querySelectorAll("button[data-s]")) b.setAttribute("aria-pressed", String(b.dataset.s === name));
  for (const pg of document.querySelectorAll(".spage")) pg.hidden = pg.dataset.s !== name;
  if (name === "engines") refreshEngines();
}

/** Core choices for the Performance page, and the speed table. */
function initPerformance() {
  const cores = navigator.hardwareConcurrency || 4;
  const choices = [[0, `All cores (${cores})`]];
  if (cores >= 4) choices.push([Math.floor(cores / 2), `Half of them (${Math.floor(cores / 2)})`]);
  for (const n of [1, 2, 4, 8]) if (n < cores && n !== Math.floor(cores / 2)) choices.push([n, n === 1 ? "One core" : `${n} cores`]);
  fill($("optThreads"), choices, threads);
  $("optThreads").onchange = () => { threads = Number($("optThreads").value); store.set("threads", String(threads)); };
  const rows = cat.all_streams.filter((s) => s.speed_cost);
  $("speedTable").innerHTML = "<tr><th>Format</th><th>Balanced</th><th>Fast</th></tr>" +
    rows.map((s) => `<tr><td>${s.name}</td><td>${s.speed_cost.balanced}</td><td>${s.speed_cost.fast}</td></tr>`).join("");
}

function initSettings() {
  for (const b of $("sTabs").querySelectorAll("button[data-s]")) b.onclick = () => showSettingsTab(b.dataset.s);
  $("sBack").onclick = () => setMode(lastMode);
  initPerformance();
  $("speedSeg").onclick = (e) => {
    const b = e.target.closest("button[data-speed]");
    if (!b || b.disabled) return;
    speed = b.dataset.speed;
    store.set("speed", speed);
    syncSpeed();
  };
  $("openSettings").onclick = () => setMode(document.body.classList.contains("tools-mode") && !$("settingsPage").hidden ? lastMode : "settings");
  $("ffBrowse").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: [{ name: "ffmpeg", extensions: ["exe"] }, { name: "All files", extensions: ["*"] }] });
    if (f) { toolPaths.ffmpeg = f; store.set("ffmpegPath", f); await pushToolPaths(); await refreshEngines(); }
  };
  $("ffReset").onclick = async () => { toolPaths.ffmpeg = ""; store.set("ffmpegPath", ""); await pushToolPaths(); await refreshEngines(); };
  $("ffFetch").onclick = () => downloadFfmpeg([$("ffFetch"), $("toolBannerFetch")], (t) => { $("ffVersion").textContent = t; });
  $("toolBannerFetch").onclick = () => downloadFfmpeg([$("ffFetch"), $("toolBannerFetch")], (t) => { $("toolBannerText").textContent = t; });
  $("masBrowse").onclick = async () => {
    const d = await dialog.open({ directory: true });
    if (d) { toolPaths.mas = d; store.set("masPath", d); await pushToolPaths(); await refreshEngines(); }
  };
  $("masReset").onclick = async () => { toolPaths.mas = ""; store.set("masPath", ""); await pushToolPaths(); await refreshEngines(); };
  $("deeBrowse").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: [{ name: "Dolby Encoding Engine", extensions: ["exe"] }] });
    if (f) { store.set("dee", f); await refreshEngine(); await refreshEngines(); }
  };
  $("deeReset").onclick = async () => { store.set("dee", ""); await refreshEngine(); await refreshEngines(); };
}

/** Downloads FFmpeg into Subwoofa's own folder, with the buttons that offered it showing progress. */
async function downloadFfmpeg(buttons, note) {
  for (const b of buttons) b.disabled = true;
  note("Downloading FFmpeg (about 115 MB)...");
  try {
    const result = await invoke("fetch_ffmpeg");
    toolPaths.ffmpeg = ""; store.set("ffmpegPath", "");
    await pushToolPaths();
    await refreshEngines();
    note(result);
  } catch (e) { note(String(e)); }
  finally { for (const b of buttons) b.disabled = false; }
}
