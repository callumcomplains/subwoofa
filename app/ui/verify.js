// Verify: checking stream and audio files.

/* ---------- Verify ---------- */

const VERIFY_FILTERS = [{ name: "Encoded audio streams and files", extensions: ["ac3", "eac3", "ec3", "dts", "dtshd", "cpt", "thd", "mlp", "truehd", "wav", "aif", "aiff", "caf", "flac", "wv", "ape", "als", "mxf", "oga", "ogg", "m4a", "tta", "dsf", "dff"] }, { name: "All files", extensions: ["*"] }];
const verify = { path: null, text: "", html: "", running: false };
const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const STATUS_TAG = { pass: "OK", fail: "FAIL", warn: "WARN", skipped: "SKIP", info: "INFO" };

function setVerifyFile(path) {
  verify.path = path;
  $("vPath").value = path;
  $("vResult").hidden = true;
  $("vSave").disabled = true;
  vLog("Ready. Choose Verify to start.");
}

function vLog(text, cls) {
  const el = $("vLog");
  el.textContent = text;
  el.className = "selectable" + (cls ? " " + cls : "");
  el.hidden = !text;
}

function renderVerify(r) {
  const failed = r.checks.filter((c) => c.status === "fail").length + r.findings.filter((f) => f.status === "fail").length;
  const warned = r.checks.filter((c) => c.status === "warn").length + r.findings.filter((f) => f.status === "warn").length;
  const v = $("vVerdict");
  v.className = "verdict " + (r.passed ? "pass" : "fail");
  v.innerHTML = r.passed
    ? `Passed<small>${esc(r.format)}. ${r.deep ? "Deep verification" : "Structure check"} found no errors${warned ? `, ${warned} warning${warned > 1 ? "s" : ""}` : ""}.</small>`
    : `Failed<small>${esc(r.format)}. ${failed} problem${failed === 1 ? "" : "s"} found${warned ? `, ${warned} warning${warned > 1 ? "s" : ""}` : ""} - see below.</small>`;
  $("vSummary").innerHTML = r.summary.map(([k, val]) => `<dt>${esc(k)}</dt><dd>${esc(val)}</dd>`).join("");
  $("vChecks").innerHTML = r.checks.map((c) => `<li class="${c.status}"><span class="tag">${STATUS_TAG[c.status]}</span><span>${esc(c.name)}</span><span class="detail">${esc(c.detail)}</span></li>`).join("");
  $("vFindingsWrap").hidden = !r.findings.length;
  $("vFindings").innerHTML = r.findings.map((f) => `<li class="finding ${f.status}"><span class="tag">${STATUS_TAG[f.status]}</span><span class="detail">${esc(f.message)}${f.offset != null ? ` <span class="at">(byte ${f.offset.toLocaleString()})</span>` : ""}</span></li>`).join("")
    + (r.findings_omitted ? `<li class="finding info"><span class="tag">INFO</span><span class="detail">and ${r.findings_omitted.toLocaleString()} more</span></li>` : "");
  const open = new Set(["Decoded audio", "Bit rate"]);
  $("vSections").innerHTML = r.sections.map((s) => `<details${open.has(s.title) ? " open" : ""}><summary>${esc(s.title)}</summary><dl class="kv selectable">${s.rows.map(([k, val]) => `<dt>${esc(k)}</dt><dd>${esc(val)}</dd>`).join("")}</dl></details>`).join("");
  $("vResult").hidden = false;
}

function initVerify() {
  $("vOpen").onclick = async () => {
    const f = await dialog.open({ multiple: false, filters: VERIFY_FILTERS });
    if (f) setVerifyFile(f);
  };
  $("vRun").onclick = async () => {
    if (!verify.path) { vLog("Choose a file first.", "bad"); return; }
    if (verify.running) return;
    verify.running = true;
    $("vRun").disabled = true;
    $("vResult").hidden = true;
    const deep = $("vDeep").checked;
    const t0 = Date.now();
    const what = deep ? "Verifying (deep, decoding the whole file)" : "Verifying";
    const tick = setInterval(() => vLog(`${what}… ${Math.round((Date.now() - t0) / 1000)} s`), 500);
    vLog(`${what}…`);
    try {
      const out = await invoke("verify_file", { path: verify.path, deep });
      verify.text = out.text;
      verify.html = out.html;
      renderVerify(out.report);
      $("vSave").disabled = false;
      vLog(`Done in ${((Date.now() - t0) / 1000).toFixed(1)} s.`);
    } catch (e) {
      vLog(String(e), "bad");
    } finally {
      clearInterval(tick);
      verify.running = false;
      $("vRun").disabled = false;
    }
  };
  $("vSave").onclick = async () => {
    const base = (verify.path || "report").replace(/\.[^.\\/]*$/, "");
    const f = await dialog.save({ defaultPath: base + "_QC.html", filters: [{ name: "QC report (web page)", extensions: ["html"] }, { name: "Text", extensions: ["txt"] }] });
    if (f) { await invoke("write_text", { path: f, text: /\.html?$/i.test(f) ? verify.html : verify.text }); vLog(`Saved ${f}`); }
  };
}
