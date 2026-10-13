// The Bitstream tab: programme information, seamless branching, secondary-audio and downmix metadata.

/* ---------- Bitstream tab ---------- */

let bsCsvPath = "";
let bsCsvPoints = [];

/** Which parts of the Bitstream tab make sense for the chosen format and layout. */
function syncBitstream() {
  const st = $("stream").value, dest = $("dest").value, lay = curLayoutId();
  const dts = ["dts", "dts-es", "dts-96-24", "dtshd-hra", "dtshd-ma", "dts-express"].includes(st);
  const container = $("ext").textContent === ".dtshd";
  const secondary = st === "dts-express" && dest === "bluray-secondary";
  const express = st === "dts-express";
  $("bsInfoWrap").hidden = !container || !dts;
  $("bsTcWrap").hidden = !container || !dts;
  $("bsSeamlessWrap").hidden = !container || !dts || express;
  $("bsWideWrap").hidden = !(lay === "7.1-wide" && ["dtshd-ma", "dtshd-hra"].includes(st));
  $("bsDialogWrap").hidden = !express;
  $("bsSecWrap").hidden = !secondary;
  $("bsMapWrap").hidden = !(secondary && (curLayout() ? curLayout().channels.length : 0) === 1);
  $("bs").hidden = [...$("bs").querySelectorAll(".bs-grid > .f")].every((n) => n.hidden);
  $("bsCsv").hidden = $("bsSeamless").value !== "csv";
  $("bsCsvNote").textContent = $("bsSeamless").value === "csv" ? (bsCsvPath ? `${bsCsvPoints.length} branch point${bsCsvPoints.length === 1 ? "" : "s"} from ${bsCsvPath.split(/[\\/]/).pop()}` : "no CSV file chosen") : "";
  syncDownmix();
  syncTabs();
}

const DMX_CH = ["C", "L", "R", "Ls", "Rs", "LFE"];
const DMX_DEFAULT = {
  primary: [3, 3, 3, 3, 3, 3],
  extra_a: [null, null, null, 3, null, null], extra_b: [null, null, null, null, 3, null],
  stereo_a: [6, 3, null, 6, null, null], stereo_b: [6, null, 3, null, 6, null],
};
const DMX_TABLES = {
  dmx51Grid: { key: "dmx51", heads: ["5.1 level", "Extra 1", "Extra 2"], cols: ["primary", "extra_a", "extra_b"] },
  dmx20Grid: { key: "dmx20", heads: ["To left", "To right"], cols: ["stereo_a", "stereo_b"] },
};

function buildDmxGrids() {
  for (const [id, t] of Object.entries(DMX_TABLES)) {
    const grid = $(id);
    grid.style.gridTemplateColumns = `3.2em repeat(${t.cols.length}, 6.5em)`;
    grid.innerHTML = `<span></span>` + t.heads.map((h) => `<span class="h">${h}</span>`).join("") +
      DMX_CH.map((ch, i) => `<span>${ch}</span>` + t.cols.map((c) => `<input type="number" min="0" max="90" step="0.5" id="${t.key}_${c}_${i}" aria-label="${ch}, ${t.heads[t.cols.indexOf(c)]}" value="${DMX_DEFAULT[c][i] ?? ""}">`).join("")).join("");
  }
}

function dmxTable(col) {
  return DMX_CH.map((_, i) => {
    const v = $(`${col.startsWith("stereo") ? "dmx20" : "dmx51"}_${col}_${i}`).value.trim();
    return v === "" ? null : Math.max(0, Math.min(90, Number(v)));
  });
}

/** The Downmix settings in the shape the encoder takes them. */
function dmxSpec() {
  const stereo = !$("dmxStereoWrap").hidden;
  return {
    primary: dmxTable("primary"), extra_a: dmxTable("extra_a"), extra_b: dmxTable("extra_b"), stereo_a: dmxTable("stereo_a"), stereo_b: dmxTable("stereo_b"),
    embed_stereo: stereo && !$("dmxEmbedWrap").hidden && $("dmxEmbed").checked,
    stereo_matrix: stereo && !$("dmxMatrixWrap").hidden && $("dmxMatrix").checked,
    lt_rt: stereo && $("dmxLtRt").checked,
  };
}

function setDmx(d) {
  d = d || {};
  for (const col of ["primary", "extra_a", "extra_b", "stereo_a", "stereo_b"]) {
    const vals = d[col] || DMX_DEFAULT[col];
    DMX_CH.forEach((_, i) => { $(`${col.startsWith("stereo") ? "dmx20" : "dmx51"}_${col}_${i}`).value = vals[i] ?? ""; });
  }
  $("dmxEmbed").checked = !!d.embed_stereo; $("dmxMatrix").checked = !!d.stereo_matrix; $("dmxLtRt").checked = !!d.lt_rt;
}

/** Which parts of the Downmix section make sense for the chosen format and layout. */
function syncDownmix() {
  const st = $("stream").value, lay = curLayout();
  const ch = lay ? lay.channels : [];
  const has = (...c) => c.every((x) => ch.includes(x));
  const five = has("C", "L", "R") && (has("Ls", "Rs") || has("Lss", "Rss"));
  const lfe = ch.includes("LFE") ? 1 : 0;
  const ma = st === "dtshd-ma", hra = st === "dtshd-hra";
  const esMatrix = lay && lay.id.startsWith("6.") && !lay.id.endsWith("premixed");
  const pair = five && ch.length === 7 + lfe;
  // Master Audio can carry a stereo set and the levels; the core streams only write the levels (into their auxiliary data)
  const coreLevels = ["dts", "dts-96-24", "dtshd-hra"].includes(st) && five && ch.length <= 8 && !esMatrix;
  const stereoOk = (ma && five && ch.length <= 8) || coreLevels;
  $("dmxStereoWrap").hidden = !stereoOk;
  $("dmxEmbedWrap").hidden = !(ma && stereoOk && curRate() <= 96000);
  $("dmxMatrixWrap").hidden = !(stereoOk && !esMatrix);
  $("dmxLtRtWrap").hidden = $("dmxMatrixWrap").hidden;
  $("dmx51Wrap").hidden = !((ma || hra) && pair);
  $("dmx20Wrap").hidden = $("dmxStereoWrap").hidden || !(($("dmxEmbed").checked && !$("dmxEmbedWrap").hidden) || ($("dmxMatrix").checked && !$("dmxMatrixWrap").hidden));
  // High Resolution Audio scales every core channel by one value
  for (let i = 1; i < 6; i++) {
    const cell = $(`dmx51_primary_${i}`);
    cell.disabled = hra;
    if (hra) cell.value = $("dmx51_primary_0").value;
  }
  $("dmx").hidden = $("dmxStereoWrap").hidden && $("dmx51Wrap").hidden;
}

/** Dropdown level (dB below full) of a speaker box; empty means off. */
function bsLevel(id) {
  const v = $(id).value.trim();
  return v === "" ? null : Math.max(0, Math.min(60, Number(v)));
}

/** The Bitstream settings in the shape the encoder takes them. */
function bitstreamSpec() {
  const seam = $("bsSeamless").value;
  return {
    program_info: $("bsInfoWrap").hidden ? "" : $("bsInfo").value,
    seamless: $("bsSeamlessWrap").hidden || seam === "off" ? "Off" : seam === "single" ? "SingleClip" : { Timecodes: bsCsvPoints },
    wide_remap: !$("bsWideWrap").hidden && $("bsWide").checked,
    downmix: dmxSpec(),
    timecode: $("bsTcWrap").hidden ? { rate: "", start: "", reference: "" } : { rate: $("bsFps").value, start: $("bsTcStart").value.trim(), reference: $("bsTcRef").value.trim() },
    secondary: {
      attenuation_db: Math.max(0, Math.min(60, Number($("bsAtt").value) || 0)),
      fade_down_secs: Math.max(0, Math.min(9, Number($("bsFadeDown").value) || 0)),
      fade_up_secs: Math.max(0, Math.min(9, Number($("bsFadeUp").value) || 0)),
      dialog_mode: !$("bsDialogWrap").hidden && $("bsDialog").checked,
      mono_map: { c: bsLevel("bsMapC"), l: bsLevel("bsMapL"), r: bsLevel("bsMapR"), ls: bsLevel("bsMapLs"), rs: bsLevel("bsMapRs"), lfe: bsLevel("bsMapLfe") },
    },
  };
}

async function chooseBranchCsv() {
  const path = await dialog.open({ multiple: false, filters: [{ name: "CSV branch points", extensions: ["csv", "txt"] }] });
  if (!path) return;
  try {
    const text = await invoke("read_log", { path });
    bsCsvPoints = text.split(/\r?\n/).map((l) => l.split(",")[0].trim()).filter(Boolean);
    bsCsvPath = path;
  } catch (e) { log(String(e), "bad"); }
  syncBitstream();
}
