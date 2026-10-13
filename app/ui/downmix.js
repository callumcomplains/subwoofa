// The Downmix tab: the matrix and the clipping check.

/* ---------- downmix & clip check ---------- */

let dm = { to: "stereo", matrix: null };

const fmtDb = (c) => (c.gain_db == null ? "off" : String(Math.round(c.gain_db * 100) / 100));

async function dmLoad(keepTarget = true) {
  const l = curLayout();
  const canTo51 = l.channels.filter((c) => c !== "LFE").length >= 6;
  const targets = [["stereo", "Stereo (Lo/Ro)"], ...(canTo51 ? [["5.1", "5.1"]] : [])];
  fill($("dmTarget"), targets, keepTarget ? dm.to : "stereo");
  dm.to = $("dmTarget").value;
  dm.matrix = await invoke("downmix_default", { layout: l.id, to: dm.to });
  $("dmResult").textContent = "";
  $("dmResult").className = "dm-result";
  dmRender();
}

function dmRender() {
  const m = dm.matrix, t = $("dmTable");
  t.innerHTML = `<thead><tr><th></th>${m.inputs.map((c) => `<th class="in">${c}</th>`).join("")}</tr></thead>`;
  const body = t.createTBody();
  m.outputs.forEach((out, o) => {
    const tr = body.insertRow();
    tr.innerHTML = `<td class="out">${out}</td>` + m.cells[o].map((c, i) =>
      `<td><input type="text" data-o="${o}" data-i="${i}" value="${fmtDb(c)}" class="${c.gain_db == null ? "off" : ""}" aria-label="${m.inputs[i]} into ${out}, dB"></td>`).join("");
  });
  t.querySelectorAll("input").forEach((inp) => inp.addEventListener("change", () => {
    const cell = dm.matrix.cells[inp.dataset.o][inp.dataset.i];
    const raw = inp.value.trim().toLowerCase();
    const v = Number(raw);
    if (raw === "" || raw === "off" || raw === "-inf") cell.gain_db = null;
    else if (Number.isFinite(v)) cell.gain_db = Math.max(-60, Math.min(12, v));
    inp.value = fmtDb(cell);
    inp.classList.toggle("off", cell.gain_db == null);
  }));
  $("dmNote").textContent = `${m.inputs.length} in → ${m.outputs.length} out · values in dB, "off" disconnects`;
}

async function dmCheck() {
  const need = curLayout().channels;
  const res = $("dmResult");
  if (!need.every((c) => inputs[c])) { res.className = "dm-result bad"; res.textContent = "Load all input files first."; return; }
  const entire = $("entire").checked;
  $("dmCheck").disabled = true;
  res.className = "dm-result";
  res.textContent = "Analysing…";
  try {
    const r = await invoke("downmix_check", { req: {
      layout: curLayoutId(), inputs, matrix: dm.matrix,
      start: entire ? null : Number($("start").value), duration: entire ? null : Number($("duration").value),
    }});
    const rate = curRate() || 48000;
    const peaks = r.channels.map((c, i) => `${c} ${r.peak_db[i] == null ? "silent" : r.peak_db[i].toFixed(2) + " dBFS"}`).join("   ");
    const secs = (f) => { const s = f / rate; return `${Math.floor(s / 60)}:${(s % 60).toFixed(2).padStart(5, "0")}`; };
    let txt = `${(r.frames / rate).toFixed(1)} s analysed · peaks: ${peaks}\n`;
    if (r.event_count === 0) { txt += "No clipping."; res.className = "dm-result ok"; }
    else {
      txt += `${r.event_count} clipping event${r.event_count === 1 ? "" : "s"}. First: ` +
        r.events.slice(0, 6).map((e) => `${r.channels[e.channel]} @ ${secs(e.frame)}`).join(", ") + ". Try Safe trim.";
      res.className = "dm-result bad";
    }
    res.textContent = txt;
  } catch (e) { res.className = "dm-result bad"; res.textContent = String(e); }
  $("dmCheck").disabled = false;
}
