// A hot folder for the Encode tab: while it is on, every complete set of channel files that lands in the chosen
// folder is queued with the current settings, once its files have stopped changing. The command line has
// `subwoofa watch` for the same without a window.

const WATCH_EVERY_MS = 5000;
const watch = { dir: null, timer: null, busy: false, seen: {}, done: new Set(), skipped: new Set() };

/** Makes a name safe for a file. */
const safeName = (name) => name.replace(/[\\/:*?"<>|]/g, "_");

async function watchTick() {
  if (watch.busy) return;
  watch.busy = true;
  try {
    if (!outDir || !curStream() || !$("outname")) { setWatchState("Choose an output folder, then it will start."); return; }
    const groups = await invoke("scan_folder", { dir: watch.dir, layout: inLayout().id });
    let waiting = 0;
    for (const g of groups) {
      if (watch.done.has(g.name)) continue;
      // sorted: the order of a set of files comes back different every time, and a fingerprint must not change with it
      const files = Object.values(g.inputs).sort();
      const stats = await invoke("path_stats", { paths: files });
      const fingerprint = JSON.stringify(stats);
      const steady = watch.seen[g.name] === fingerprint;
      watch.seen[g.name] = fingerprint;
      if (!steady) { waiting++; continue; }
      const safe = safeName(g.name);
      const output = outDir + "\\" + safe + $("ext").textContent;
      const [there] = await invoke("path_stats", { paths: [output] });
      watch.done.add(g.name);
      if (there) { log(`Watch: ${g.name} is already encoded, skipped.`); continue; }
      try {
        await invoke("queue_add", { verify: autoVerify, spec: jobSpec(g.inputs, safe) });
        log(`Watch: queued ${g.name}.`, "ok");
      } catch (e) { log(`Watch: ${g.name}: ${e}`, "bad"); }
    }
    setWatchState(waiting ? `Watching ${watch.dir} (${waiting} set${waiting === 1 ? "" : "s"} still arriving)` : `Watching ${watch.dir}`);
  } catch (e) {
    setWatchState(String(e));
  } finally {
    watch.busy = false;
  }
}

function setWatchState(text) {
  $("watchBtn").title = text;
}

async function toggleWatch() {
  if (watch.timer) {
    clearInterval(watch.timer);
    watch.timer = null;
    $("watchBtn").textContent = "Watch folder…";
    $("watchBtn").setAttribute("aria-pressed", "false");
    log("Stopped watching the folder.");
    return;
  }
  if (!outDir) { log("Choose an output folder first.", "bad"); return; }
  const dir = await dialog.open({ directory: true, multiple: false });
  if (!dir) return;
  Object.assign(watch, { dir, seen: {}, done: new Set() });
  $("watchBtn").textContent = "Stop watching";
  $("watchBtn").setAttribute("aria-pressed", "true");
  log(`Watching ${dir} for sets of ${inLayout().channels.length} channel files. New sets are queued with the current settings.`, "ok");
  watchTick();
  watch.timer = setInterval(watchTick, WATCH_EVERY_MS);
}
