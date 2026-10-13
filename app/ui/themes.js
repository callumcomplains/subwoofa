// Colour themes and the custom theme editor.

/* ---------- themes ---------- */

const THEMES = [
  ["amber", "Studio Amber"], ["graphite", "Graphite"], ["midnight", "Midnight"], ["ocean", "Ocean"], ["moss", "Moss"],
  ["rose", "Rose Dusk"], ["terminal", "Terminal"], ["paper", "Paper"], ["daylight", "Daylight"], ["contrast", "High Contrast"],
];
const CUSTOM_KEYS = ["ground", "panel", "rule", "ink", "dim", "lamp"];

/** The six colours a theme block gives, read back from the stylesheet. */
function themeColours(name) {
  const probe = document.createElement("div");
  probe.dataset.theme = name;
  probe.style.display = "none";
  document.body.append(probe);
  const cs = getComputedStyle(probe);
  const out = {};
  for (const k of CUSTOM_KEYS) out[k] = cs.getPropertyValue("--" + k).trim();
  probe.remove();
  return out;
}

function luminance(hex) {
  const n = parseInt(hex.slice(1), 16), c = [n >> 16, (n >> 8) & 255, n & 255].map((v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; });
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
}

/** Everything else a theme needs, worked out from the six colours. */
function customVars(c) {
  const light = luminance(c.ground) > 0.4;
  return {
    "--ground": c.ground, "--panel": c.panel, "--rule": c.rule, "--ink": c.ink, "--dim": c.dim, "--lamp": c.lamp,
    "--hi": "color-mix(in srgb, var(--panel) 90%, var(--ink))",
    "--screen": light ? "color-mix(in srgb, var(--panel) 55%, #fff)" : "color-mix(in srgb, var(--ground) 65%, #000)",
    "--rule-hi": "color-mix(in srgb, var(--rule) 65%, var(--ink))",
    "--lamp-ink": luminance(c.lamp) > 0.35 ? "#14110a" : "#ffffff",
    "color-scheme": light ? "light" : "dark",
  };
}

function loadCustom() {
  try { const c = JSON.parse(store.get("customTheme") || "null"); if (c && c.colors) return c; } catch { /* fall through */ }
  return { base: "amber", colors: themeColours("amber") };
}

function saveCustom(c) {
  c.vars = customVars(c.colors);
  store.set("customTheme", JSON.stringify(c));
  return c;
}

function clearInlineTheme() {
  const s = document.documentElement.style;
  for (const k of ["--ground", "--panel", "--rule", "--ink", "--dim", "--lamp", "--hi", "--screen", "--rule-hi", "--lamp-ink", "color-scheme"]) s.removeProperty(k);
}

function applyTheme(name) {
  if (name !== "custom" && !THEMES.some((t) => t[0] === name)) name = "amber";
  clearInlineTheme();
  document.documentElement.dataset.theme = name;
  if (name === "custom") for (const [k, v] of Object.entries(saveCustom(loadCustom()).vars)) document.documentElement.style.setProperty(k, v);
  store.set("theme", name);
  for (const b of $("themeGrid").children) b.setAttribute("aria-checked", String(b.dataset.theme === name));
  $("customEditor").hidden = name !== "custom";
}

function paintCustomCard() {
  const card = $("themeGrid").querySelector('[data-theme="custom"]');
  if (!card) return;
  const c = loadCustom();
  for (const [k, v] of Object.entries(customVars(c.colors))) card.style.setProperty(k, v);
}

function buildThemeGrid() {
  const all = [...THEMES, ["custom", "Custom"]];
  $("themeGrid").innerHTML = all.map(([id, label]) =>
    `<button type="button" role="radio" class="theme" data-theme="${id}" aria-checked="false"><span class="sw"><i></i><b></b><u></u><s></s></span><span class="n">${label}</span></button>`).join("");
  $("themeGrid").onclick = (e) => { const b = e.target.closest(".theme"); if (b) { applyTheme(b.dataset.theme); syncCustomEditor(); } };
  fill($("customBase"), THEMES.map(([id, label]) => [id, label]), loadCustom().base);
  paintCustomCard();
  applyTheme(document.documentElement.dataset.theme);
  syncCustomEditor();
  for (const input of $("customEditor").querySelectorAll("input[type=color]")) {
    input.oninput = () => {
      const c = loadCustom();
      c.colors[input.dataset.var] = input.value;
      saveCustom(c);
      applyTheme("custom");
      paintCustomCard();
    };
  }
  $("customBase").onchange = () => resetCustom($("customBase").value);
  $("customReset").onclick = () => resetCustom($("customBase").value);
  $("customCopy").onclick = async () => {
    try { await navigator.clipboard.writeText(JSON.stringify(loadCustom().colors)); flash($("customCopy"), "Copied"); } catch { flash($("customCopy"), "Blocked"); }
  };
  $("customPaste").onclick = async () => {
    try {
      const t = JSON.parse(await navigator.clipboard.readText());
      const c = loadCustom();
      for (const k of CUSTOM_KEYS) if (/^#[0-9a-f]{6}$/i.test(t[k] || "")) c.colors[k] = t[k];
      saveCustom(c); applyTheme("custom"); syncCustomEditor(); paintCustomCard();
      flash($("customPaste"), "Pasted");
    } catch { flash($("customPaste"), "No theme text"); }
  };
}

function resetCustom(base) {
  const c = { base, colors: themeColours(base) };
  saveCustom(c);
  applyTheme("custom");
  syncCustomEditor();
  paintCustomCard();
}

function syncCustomEditor() {
  const c = loadCustom();
  for (const input of $("customEditor").querySelectorAll("input[type=color]")) input.value = c.colors[input.dataset.var];
  $("customBase").value = c.base;
}

function flash(btn, text) {
  const was = btn.textContent;
  btn.textContent = text;
  setTimeout(() => { btn.textContent = was; }, 1200);
}
