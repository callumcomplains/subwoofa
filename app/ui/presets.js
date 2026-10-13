// Presets: saving and applying the whole page as a .swpreset file.

/* ---------- presets (.swpreset, plain JSON) ---------- */

const PRESET_FILTER = [{ name: "Subwoofa preset", extensions: ["swpreset"] }];

function presetState() {
  return {
    app: "subwoofa", version: 1,
    dest: $("dest").value, stream: $("stream").value, layout: curLayoutId(), sample_rate: curRate(),
    bitrate: $("bitrate").value === "" ? null : Number($("bitrate").value),
    pcm_format: $("pcmFormat").value, core_on: $("coreOn").checked, dialnorm_on: $("dialnormOn").checked, dialnorm: Number($("dialnorm").value),
    es_phase_shift: $("esShift").checked, rear_minus_3db: $("rear3").checked,
    bitstream: { fps: $("bsFps").value, tc_start: $("bsTcStart").value, tc_ref: $("bsTcRef").value, info: $("bsInfo").value, seamless: $("bsSeamless").value, csv_path: bsCsvPath, wide: $("bsWide").checked, dialog: $("bsDialog").checked,
      att: $("bsAtt").value, fade_down: $("bsFadeDown").value, fade_up: $("bsFadeUp").value,
      map: { c: $("bsMapC").value, l: $("bsMapL").value, r: $("bsMapR").value, ls: $("bsMapLs").value, rs: $("bsMapRs").value, lfe: $("bsMapLfe").value } },
    downmix: dmxSpec(),
    unlocked, outname: $("outname").value, output_dir: outDir,
  };
}

async function applyPreset(p) {
  if (p.app !== "subwoofa") throw new Error("not a Subwoofa preset");
  setUnlocked(!!p.unlocked);
  if ([...$("dest").options].some((o) => o.value === p.dest)) $("dest").value = p.dest;
  await cascade("dest");
  if ([...$("stream").options].some((o) => o.value === p.stream)) $("stream").value = p.stream;
  await cascade("stream", p.layout);
  if (p.sample_rate && [...$("rate").options].some((o) => o.value === String(p.sample_rate))) {
    $("rate").value = String(p.sample_rate);
    $("ratecustom").hidden = true;
    await cascade("rate", p.layout);
  }
  $("coreOn").checked = p.core_on !== false;
  $("esShift").checked = !!p.es_phase_shift;
  $("rear3").checked = !!p.rear_minus_3db;
  const bs = p.bitstream || {};
  $("bsInfo").value = bs.info || "";
  $("bsFps").value = bs.fps || ""; $("bsTcStart").value = bs.tc_start || ""; $("bsTcRef").value = bs.tc_ref || "";
  $("bsSeamless").value = ["off", "single", "csv"].includes(bs.seamless) ? bs.seamless : "off";
  $("bsWide").checked = !!bs.wide;
  $("bsDialog").checked = !!bs.dialog;
  $("bsAtt").value = bs.att ?? 6; $("bsFadeDown").value = bs.fade_down ?? 0; $("bsFadeUp").value = bs.fade_up ?? 0;
  const m = bs.map || { c: "0" };
  for (const [k, id] of [["c", "bsMapC"], ["l", "bsMapL"], ["r", "bsMapR"], ["ls", "bsMapLs"], ["rs", "bsMapRs"], ["lfe", "bsMapLfe"]]) $(id).value = m[k] ?? "";
  setDmx(p.downmix);
  bsCsvPath = ""; bsCsvPoints = [];
  if (bs.seamless === "csv" && bs.csv_path) {
    try {
      const text = await invoke("read_log", { path: bs.csv_path });
      bsCsvPoints = text.split(/\r?\n/).map((l) => l.split(",")[0].trim()).filter(Boolean);
      bsCsvPath = bs.csv_path;
    } catch (e) { /* the file moved: the note says no file is chosen */ }
  }
  syncBitstream();
  if (p.pcm_format) $("pcmFormat").value = p.pcm_format;
  $("dialnormOn").checked = !!p.dialnorm_on;
  $("dialnorm").disabled = !$("dialnormOn").checked;
  if (Number.isFinite(p.dialnorm)) $("dialnorm").value = p.dialnorm;
  syncDialnorm();
  renderBitrate("preset");
  if (p.bitrate != null && !$("bitrate").disabled) $("bitrate").value = p.bitrate;
  if (p.outname) { $("outname").value = p.outname; nameAuto = false; }
  if (p.output_dir) { outDir = p.output_dir; $("outdir").value = p.output_dir; }
  syncChips();
  update();
}

function setUnlocked(v) {
  unlocked = v;
  store.set("unlocked", v ? "1" : "0");
  $("optUnlocked").checked = v;
  $("unlockedTag").hidden = !v;
  $("bitrate").removeAttribute("min");
}
