// The tabs of the main card, the (i) marks and the format plates.

/* ---------- tabs of the main card ---------- */

function showTab(id) {
  for (const p of ["inputsPanel", "bs", "dmx"]) $(p).classList.toggle("active", p === id);
  for (const b of $("mainTabs").children) b.setAttribute("aria-pressed", String(b.dataset.tab === id));
}

/** A tab is offered only while its panel applies to the chosen format and layout. */
function syncTabs() {
  let active = [...$("mainTabs").children].find((b) => b.getAttribute("aria-pressed") === "true")?.dataset.tab || "inputsPanel";
  for (const b of $("mainTabs").children) b.hidden = b.dataset.tab !== "inputsPanel" && $(b.dataset.tab).hidden;
  $("mainTabs").hidden = $("bs").hidden && $("dmx").hidden;
  if ($(active).hidden) active = "inputsPanel";
  showTab(active);
}

/* ---------- (i) marks ---------- */

// [selector, text]. A selector starting with @ names the label itself; any other names a control, and the mark
// goes after its label. Anything that changes with the choices (what a stream type contains, for one) is added
// to the end with setTip.
const INFO = [
  ["@#pvTitle", "Encodes about 20 seconds from the middle of your programme with these exact settings, decodes it again, and lets you flip between the source, the result, and what the encoder took away. Everything is mixed to stereo the same way, so the only difference you hear is the format. 1, 2 and 3 switch, space plays."],
  ["@.format .dh", "Everything about the stream itself: format, rate, bit rate and the loudness tag."],
  ["#dest", "Where this file is going. It sets the rules: which formats, rates and layouts are allowed. Blu-ray and DVD are strict, Generic is relaxed, and Audio files will take nearly anything."],
  ["#stream", "The audio format. Lossless keeps every bit; lossy keeps what you probably won't miss. Pick whatever your disc or player asks for."],
  ["#rate", "Samples per second. Match your source: 48 kHz for video, 96 kHz and up for the audiophile corner. A source at another rate gets converted."],
  ["#bitrate", "Kilobits per second for the lossy part. More sounds better and weighs more. The range beside the label is what this format allows; the buttons are the usual suspects."],
  ["#x96Core", "Adds an extension so a decoder that only reads the core still gets the full 96 kHz band."],
  ["#coreOn", "The lossy core lets players without DTS-HD decode something. Switch it off for lossless data only: smaller, but picky players may stay silent."],
  ["#dialnormMeasure", "Reads your input files and sets the dialnorm to their integrated loudness (BS.1770). Dolby asks for the loudness of the dialogue; a whole film mix is a fair stand-in when the dialogue sits where it should. Over a span if you set one."],
  ["#dialnormOn", "Dialogue normalisation: the average loudness of your speech, stored so players can level different titles. Don't know your figure? Leave it off. Film mixes often sit around −27."],
  ["#dtsOpts", "ES phase shift keeps the rear centre from cancelling against the other rears. The −3 dB option lowers the rears to match a matrixed surround."],
  ["#pcmFormat", "How each sample is stored in the file: integer or float, 16 to 64 bits, or one-bit DSD for the brave."],
  ["@.layouts .dh", "The speaker arrangement. Pick a family like 5.1, then the exact variant. The display shows who plays where, and your input files follow the same layout."],
  ["@#inputsPanel .inputs-head h2", "One mono file per channel. Drop the left file anywhere and Subwoofa finds its siblings, or import one multichannel file and watch every row fill itself."],
  ["#srcLayout", "For when your files don't match the output layout. NHK 22.2 sources get mixed down to whatever you picked."],
  ["#entire", "Untick to encode just a slice: choose where it starts and how long it lasts. Good for a quick test before you commit an hour of audio."],
  ["@#dm summary", "See what a downmix does to your levels before you ship it. Check clipping tells you whether the worst case goes over full scale; Safe trim turns it down just enough."],
  ["#bsInfo", "Free text stored in the .dtshd file: a title, notes, a message to future you. Nobody hears it."],
  ["#bsFps", "The video frame rate and start time stored in the file, so authoring tools place the audio exactly. The reference time is where the programme was cut from."],
  ["#bsSeamless", "Lets a Blu-ray player hop between versions of a film without a hiccup, by making the stream restart cleanly at chosen points. Several branch points? Give a CSV of timecodes, one per line."],
  ["#bsWide", "Adds the data that lets a decoder play wide front channels on a side-rear layout or a plain 5.1 system."],
  ["#bsDialog", "Codes only the lower half of the frequency range. Plenty for speech, and it saves bits."],
  ["#bsAtt", "How far the main film audio is turned down while this secondary track speaks."],
  ["#bsFadeDown", "Seconds the main audio takes to sink to that lower level when the commentary starts."],
  ["#bsFadeUp", "Seconds the main audio takes to come back when the commentary ends."],
  ["@#bsMapWrap .cap", "Where a mono commentary plays, in dB below full level. Empty means off. Centre at 0 is the sensible default."],
  ["#dmxEmbed", "Puts a ready-made stereo mix inside the file, so a stereo player doesn't have to fold the surround down itself. The file gets bigger."],
  ["#dmxMatrix", "Stores your stereo mixing levels in the stream as information. Decoders may use them when they downmix."],
  ["#dmxLtRt", "Labels the stereo mix as Lt/Rt (surround-encoded, for a matrix decoder) instead of plain Lo/Ro."],
  ["@#dmx51Wrap > label", "How a 7.x stream plays as 5.1. The first column is the gain of each 5.1 channel; Extra 1 and Extra 2 say how much of the two extra channels joins it. Numbers are dB below full level, and empty means not mixed in."],
  ["@#dmx20Wrap > label", "How the stream plays as stereo: how much of each channel goes to the left and right outputs, in dB below full level. Empty means none."],
  ["@.queue-head h2", "Encodes wait here, run in turn and get verified. Close the app and they go away."],
  ["@.tools-info .dh", "What's inside the stream. Handy for answering \"what even is this file?\""],
  ["@.op[data-op=trim] .op-title", "Keeps the part between two times, cut on frame boundaries. TrueHD and MLP start at the nearest earlier restart point, because that's how they are."],
  ["@.op[data-op=split] .op-title", "Cuts the stream at each time and writes numbered files next to the name you choose."],
  ["@.op[data-op=append] .op-title", "Joins streams of the same format and sample rate end to end. The open stream goes first."],
  ["@.op[data-op=replace] .op-title", "Swaps a stretch of the open stream for another one (same format, rate and channels) starting at the time you give, or inserts it without removing anything. Cuts land on frames a decoder can start from."],
  ["@.op[data-op=restripe] .op-title", "Relabels the start time and frame rate in a .dtshd. The audio isn't touched, and the reference time stays put. Needs a timecode chunk, which Subwoofa and the DTS encoder both write."],
  ["@.op[data-op=silence] .op-title", "Adds whole frames of silence to the start and/or end of a .dtshd, so the length lands within one frame of what you asked. Needs a stored silent frame."],
  ["@.op[data-op=verify] .op-title", "Checks the open stream: structure and checksums. Deep mode adds a full decode and a look at every header."],
  ["#normOn", "Applies gain so the whole programme lands on this loudness: -23 LUFS for EBU R 128 broadcast, -24 for ATSC A/85, -27 and below for film. The true peak is kept under -1 dBTP, so a very peaky mix may come out a little quieter than asked. Needs a format Subwoofa encodes itself."],
  ["#tDnNew", "Changes the dialogue level a decoder applies, without re-encoding: only the level and the checksums are rewritten, so the audio is bit for bit what it was. Handy when a stream was made at the wrong level, or you measured it afterwards."],
  ["#vLayout", "The channel layout of the source. Pick one channel file and Subwoofa finds the others by name (Name_L.wav, Name_R.wav ...), or pick a single multichannel file. The result says whether the encode is bit-identical (lossless) or how close it is (lossy)."],
  ["#vStart", "Leave From and Length empty when the encode covers the whole source. If you encoded only part of it (a preview, or a start and length), say where that part begins and how long it is."],
  ["#vDeep", "Also decodes the whole file, measures the audio, asks DTS's own verifier if it's installed, and lists every header. Slower, more suspicious."],
  ["@#atmosTitle", "Atmos is encoded by the Dolby Encoding Engine, a copy you're licensed to use; Subwoofa only drives it and checks what comes out. Masters must come from Dolby's tools (DAMF .atmos, or an ADM BWF .wav), because the engine refuses anything else. A plain channel bed (.wav, up to 7.1.4) makes Dolby Digital Plus Atmos."],
  ["#aDialnorm", "Average speech loudness, −31 to −1. Leave it at 0 and the engine measures it for you."],
  ["#optUnlocked", "FOR ADVANCED USERS ONLY. Switches off the spec checks. Any rate, any layout, any format to any destination. The encoder may say no, and real hardware may too."],
  ["#optVerify", "After each encode: lossless files are decoded and compared with the source (MD5); lossy ones must decode cleanly; audio files get a structure check and a full decode."],
  ["#optDolbyEnc", "Sends Dolby Digital (AC-3) and Digital Plus (E-AC-3) jobs to Dolby's own encoder in the Dolby Encoding Engine instead of ours or FFmpeg's. It measures the dialogue itself and sets the dialnorm from that, so the dialnorm you give is not used. Needs the engine, and a mono, stereo, 5.1 or 7.1 layout."],
  ["#dnDolby", "Uses the Dolby Encoding Engine's loudness meter, which finds the dialogue and measures that. A dialnorm describes the dialogue level, so this is the better figure when the mix has loud music or effects. Off measures the whole programme."],
  ["#optNative", "Where Subwoofa has its own encoder (AC-3, E-AC-3, TrueHD, MLP, the DTS family), use it instead of FFmpeg. Formats FFmpeg can't make always use ours."],
  ["#optDownmix", "Native TrueHD only: adds a stereo substream so stereo-only players get a proper downmix. Costs roughly 5 to 8 % more file."],
  ["#optConc", "How many encodes run at once. More is faster on a big machine and louder on a small one."],
  ["#optThreads", "How many processor cores one encode may use. All cores is fastest. One core is slow and gentle: the machine stays usable. Several encodes at once share the cores."],
  ["@#speedBox .cap", "How hard lossless encoders search for a smaller file. Fast and Balanced finish sooner and can leave the file a little larger; Best tries everything. It changes nothing for lossy formats and nothing about whether the file is correct."],
  ["#optOrder", "Some multichannel files list their channels in an unexpected order. Auto uses film order for 7.1; the others follow the file's speaker mask, or the order of the layout."],
  ["@.engine:nth-of-type(1) .wordmark", "Reads and converts audio Subwoofa can't read itself, and makes the formats it doesn't encode natively. Leave this on automatic to search your PATH, or point it at ffmpeg.exe (or its folder) to use a particular build."],
  ["@.engine:nth-of-type(2) .wordmark", "The folder that holds DTSHDVerify.exe. Subwoofa uses DTS's own verifier as a second opinion in deep checks. Automatic looks in the default install folder."],
  ["@.engine:nth-of-type(3) .wordmark", "dee.exe, the Dolby Encoding Engine, which does the actual Atmos encoding. You supply your own licensed copy."],
  ["@.spage[data-s=appearance] .sh", "Pick a look, or mix your own under Custom. Your choice is remembered."],
  ["@#customEditor .sub", "Six colours are enough. The rest (raised panels, screens, outlines, the text on the accent) is worked out from them."],
  ["@.spage[data-s=presets] .sh", "A preset remembers format, layout, rate, bit rate, dialnorm and every Bitstream and Downmix choice. Not your input files."],
];


function composeTip(key) {
  const x = TIP_EXTRA[key];
  return TIP_BASE[key] + (x ? " " + x : "");
}

/** Add to the explanation behind a control's (i) (it may change as you pick things). */
function setTip(key, text) {
  TIP_EXTRA[key] = text;
  const icon = document.querySelector(`.info[data-key="${key}"]`);
  if (icon && TIP_BASE[key] !== undefined) icon.dataset.tip = composeTip(key);
}

function addInfos() {
  for (const [sel, text] of INFO) {
    let host;
    if (sel.startsWith("@")) host = document.querySelector(sel.slice(1));
    else {
      const el = document.querySelector(sel);
      const f = el && el.closest(".f, .chk, .opt, .srow, label");
      host = f && f.querySelector(":scope > span > b, :scope > b, :scope > span, :scope > label > span, span");
    }
    if (!host || host.querySelector(".info")) continue;
    TIP_BASE[sel] = text;
    const i = document.createElement("i");
    i.className = "info"; i.tabIndex = 0; i.textContent = "i"; i.dataset.key = sel; i.dataset.tip = composeTip(sel);
    i.setAttribute("aria-label", "More information");
    host.append(i);
  }
}

function initTips() {
  const tip = $("tip");
  const show = (el) => {
    tip.textContent = el.dataset.tip;
    tip.classList.add("on");
    const r = el.getBoundingClientRect(), t = tip.getBoundingClientRect();
    const x = Math.max(8, Math.min(r.left + r.width / 2 - t.width / 2, innerWidth - t.width - 8));
    const y = r.bottom + 8 + t.height > innerHeight ? r.top - t.height - 8 : r.bottom + 8;
    tip.style.left = x + "px"; tip.style.top = Math.max(8, y) + "px";
  };
  const hide = () => tip.classList.remove("on");
  document.addEventListener("mouseover", (e) => { const el = e.target.closest?.(".info"); if (el) show(el); });
  document.addEventListener("mouseout", (e) => { if (e.target.closest?.(".info")) hide(); });
  document.addEventListener("focusin", (e) => { const el = e.target.closest?.(".info"); if (el) show(el); });
  document.addEventListener("focusout", hide);
  document.addEventListener("scroll", hide, true);
  // a path box opens its chooser on a double click, as the file rows already do
  document.addEventListener("dblclick", (e) => {
    const box = e.target.closest?.("input[data-opens]");
    if (box) $(box.dataset.opens).click();
  });
}

/* ---------- format and destination plates ---------- */



function setPlate(el, file, alt) {
  el.innerHTML = file ? `<img src="logos/${file}" alt="">` : "";
  if (file) el.firstChild.alt = alt;
}

function updatePlate() {
  const st = $("stream").value;
  setPlate($("fmtPlate"), st ? `${st}.svg` : "", $("stream").selectedOptions[0]?.textContent || "");
  const d = $("dest").value;
  setPlate($("destPlate"), d ? `dest-${d}.svg` : "", $("dest").selectedOptions[0]?.textContent || "");
}
