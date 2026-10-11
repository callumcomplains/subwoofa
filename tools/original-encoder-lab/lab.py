"""Headless driver for the original DTS-HD Master Audio Suite encoder (DtsJobQueue.exe).

Pipeline for one case:
  1. fill the job XML (a template taken from a real GUI job) with the case's settings,
  2. have the encoder's own Java code (GenCfg) turn the XML into the .cfg file,
  3. send the engine the same command line the GUI would (framed over localhost:4444),
  4. wait for the engine's log, then return the output path.
"""
import json, os, re, socket, subprocess, sys, time, shutil

LAB = os.path.dirname(os.path.abspath(__file__))
MAS = r"C:\Program Files (x86)\DTS\MAS-SAS"
JAVA32 = r"C:\Program Files (x86)\Eclipse Adoptium\jre-8.0.472.8-hotspot\bin\java.exe"
TEMPLATE = os.path.join(LAB, "template.xml")
HOST, PORT = "127.0.0.1", 4444

# the encoder's own numbering (Constants.java)
DEST = {"bd": 1, "bd2": 2, "dvd": 3, "cd": 4, "dece": 5}
STREAM = {"express": 1, "core": 2, "core96": 3, "es": 4, "hra": 5, "ma": 6}
LAYOUT = {  # symbolic ids and the order the engine wants the input files
    "1.0": 4, "1.1": 5, "2.0lt": 6, "2.0": 7, "2.1lt": 8, "2.1": 9, "3.0s": 10, "3.0": 11, "3.1s": 12, "3.1": 13,
    "4.0cs": 14, "4.0": 15, "4.1cs": 16, "4.1": 17, "5.0": 18, "5.1": 19,
    "6.0m": 20, "6.1m": 21, "6.0d": 22, "6.1d": 23, "7.0": 24, "7.1w": 25, "7.1oh": 26, "7.1ch": 27, "7.1sr": 28,
    "7.1hs": 29, "7.1h": 30, "7.1": 31, "sa1.0": 1, "sa2.0": 2, "sa5.1": 3,
}


def frame(text):
    b = text.encode("utf-8")
    return ("%04d" % len(b)).encode() + b


class Engine:
    """One long-lived connection to DtsJobQueue.exe. The engine quits when its client disconnects (the GUI keeps its
    socket open for the whole session), so every case of a batch goes through the same socket."""
    sock = None
    inbox = []

    @classmethod
    def process_running(cls):
        out = subprocess.run(["tasklist", "/FI", "IMAGENAME eq DtsJobQueue.exe", "/NH"], capture_output=True, text=True).stdout
        return "DtsJobQueue.exe" in out

    @classmethod
    def connect(cls):
        if cls.sock is not None:
            return
        for attempt in range(3):
            if not cls.process_running():
                subprocess.Popen([os.path.join(MAS, "DtsJobQueue.exe")], cwd=MAS, creationflags=0x08000000,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            for _ in range(30):
                try:
                    cls.sock = socket.create_connection((HOST, PORT), timeout=2)
                    cls.sock.settimeout(0.2)
                    return
                except OSError:
                    time.sleep(1)
            # a stale process that no longer listens: kill it and start afresh
            subprocess.run(["taskkill", "/F", "/IM", "DtsJobQueue.exe"], capture_output=True)
            time.sleep(2)
        raise RuntimeError("engine did not start")

    @classmethod
    def drain(cls):
        """Collect any messages the engine has sent."""
        try:
            while True:
                d = cls.sock.recv(4096)
                if not d:
                    cls.sock = None
                    break
                cls.inbox.append(d.decode("utf-8", "replace"))
        except socket.timeout:
            pass

    @classmethod
    def close(cls):
        if cls.sock:
            cls.sock.close()
            cls.sock = None
            for _ in range(20):  # the engine quits when its client disconnects
                if not cls.process_running():
                    break
                time.sleep(0.5)


def ensure_engine():
    Engine.connect()


def set_tag(xml, tag, value):
    value = str(value).replace("&", "&amp;").replace("<", "&lt;")
    pat = re.compile(r"<%s>.*?</%s>|<%s/>" % (tag, tag, tag), re.S)
    if pat.search(xml):
        return pat.sub(lambda m: "<%s>%s</%s>" % (tag, value, tag), xml, count=1)
    return xml.replace("</job>", "  <%s>%s</%s>\n</job>" % (tag, value, tag))


def bool_(b):
    return "true" if b else "false"


FPS = {"23.976": "23.976", "24": "24", "25": "25", "29.97": "29.97", "30": "30"}


def build_xml(spec, outdir, name, files):
    x = open(TEMPLATE, encoding="utf-8").read()
    d, s = DEST[spec["dest"]], STREAM[spec["stream"]]
    layout = spec["layout"]
    kbps = spec.get("kbps", 0)
    kw = dict(
        isLosslessSelected=s == 6, isPrimaryAudioSelected=d in (1, 3, 4, 5), isSecondaryAudioSelected=d == 2,
        isLBRSelected=s == 1, isDVDSelected=d == 3, isCDSelected=d == 4, isDECESelected=d == 5,
        isMediaTypeSelectedCD=d == 4, isMediaTypeSelectedDVD=d == 3, isMediaTypeSelectedBD=d == 1,
        isMediaTypeSelectedBDSecondaryAudio=d == 2, isMediaTypeSelectedDECEAudio=d == 5,
        bitrate=kbps * 1000, samplerate=spec["rate"], bitwidth=spec.get("bits", 24),
        dialognormalizationnounits=spec.get("dialnorm", -31), usedialognormalization=True,
        esphaseshift=spec.get("phase_shift", False), is51ES=spec.get("es51", False),
        isPremixed=spec.get("premixed", s == 6 and layout in ("6.0m", "6.1m")), useLFE=layout in LFE_LAYOUTS,
        attenuation=spec.get("attenuate_rear", False), use9624=spec.get("use9624", False),
        framerate=spec.get("fps", "24"), encodeEntireFile=True,
        timecodestarttime="00:00:00:00", timecodeendtime=spec.get("tc_end", "00:00:10:00"),
        timecodeencodefrom="00:00:00:00", timecodeencodeto=spec.get("tc_end", "00:00:10:00"),
        savetodirectory=outdir + "\\", savetofilename=name,
        programinfo=spec.get("programinfo", ""),
        usewideremapping=spec.get("wide", False),
        useexpressdialogmode=spec.get("dialog_mode", False),
        usePrimaryAudioAttenuation=spec.get("pa_att", False),
        paMetadataPrimaryValue=spec.get("pa_att_db", "6.0"),
        paMetadataFadeDownValue=spec.get("fade_down", "2.0"), paMetadataFadeUpValue=spec.get("fade_up", "2.0"),
        useBDSecondaryAudioMetaData=d == 2,
        isEmbeddedDownmix=spec.get("embed20", False), is2ChDmixLtRt=spec.get("embed20_ltrt", False),
        use20Downmix=spec.get("use20", False), use51Downmix=spec.get("use51", layout.startswith("7.") or layout in ("6.0d", "6.1d")),
        useLegacyMatrix=spec.get("legacy_matrix", False),
        seamlessbranching_singleclip=spec.get("seamless_single", False),
        downmixSaturationCheck=spec.get("sat_check", False),
    )
    for k, v in kw.items():
        x = set_tag(x, k.replace("seamlessbranching_singleclip", "seamlessbranching-singleclip"),
                    bool_(v) if isinstance(v, bool) else v)
    for k, v in spec.get("xml", {}).items():
        x = set_tag(x, k, bool_(v) if isinstance(v, bool) else v)
    x = set_tag(x, "inputfileorder", " " + "  ".join('"%s"' % f for f in files) + " ")
    x = set_tag(x, "residualdata", "-r1")
    return x


LFE_LAYOUTS = {"1.1", "2.1lt", "2.1", "3.1s", "3.1", "4.1cs", "4.1", "5.1", "6.1m", "6.1d", "7.1w", "7.1oh", "7.1ch",
               "7.1sr", "7.1hs", "7.1h", "7.1", "sa5.1"}


def stems_for(rate):
    """Folder with the ten-second mono stems at this sample rate (created on demand)."""
    src = os.path.join(LAB, "stems")
    dst = src if rate == 96000 else os.path.join(LAB, "stems%d" % rate)
    if rate != 96000 and not os.path.isdir(dst):
        os.makedirs(dst)
    for f in os.listdir(src):
        if not f.endswith(".wav"):
            continue
        if rate == 96000:
            continue
        out = os.path.join(dst, f)
        if not os.path.exists(out):
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", os.path.join(src, f), "-ar", str(rate), "-c:a",
                            "pcm_s24le", out], check=True)
    return dst


# cfg channel name -> stem file, per layout family (the 7.1 stems are Lss/Rss/Lsr/Rsr)
def channel_files(layout, cfg_text, stem_dir):
    names = re.findall(r"^\s*CHANNEL\s+(\w+)\s", cfg_text, re.M)
    out = []
    for n in names:
        stem = {"l": "L", "r": "R", "c": "C", "lfe": "LFE", "sl": "Lss", "sr": "Rss", "es": "Lsr", "esb": "Rsr", "s": "Cs"}[n]
        if n == "es" and layout in ("6.0d", "6.1d", "6.0m", "6.1m", "7.1oh", "7.1ch"):
            stem = "Cs"
        if n == "sl" and layout in ("5.0", "5.1", "3.0s", "3.1s", "4.0cs", "4.1cs", "4.0", "4.1"):
            stem = "Lss"
        out.append(os.path.join(stem_dir, stem + ".wav"))
    return out


def send(cmd, wait=1.0):
    Engine.connect()
    Engine.inbox = []
    Engine.sock.sendall(frame(cmd))
    time.sleep(wait)
    Engine.drain()
    return list(Engine.inbox)


def wait_done(outdir, name, timeout):
    """Wait for the engine's log; return (ok, log text)."""
    end = time.time() + timeout
    while time.time() < end:
        for f in os.listdir(outdir):
            if f.startswith(name) and (f.endswith("_log.txt") or f.endswith(".txt")):
                try:
                    t = open(os.path.join(outdir, f), encoding="utf-8", errors="replace").read()
                except OSError:
                    continue
                if "Encode completed successfully" in t:
                    return True, t
                if re.search(r"Error|failed|Canceled", t):
                    return False, t
        time.sleep(0.5)
    return False, "timeout"


def run_case(spec, outroot=None, timeout=600):
    ensure_engine()
    name = spec["name"]
    outdir = os.path.join(outroot or os.path.join(LAB, "out"), spec.get("group", "misc"))
    os.makedirs(outdir, exist_ok=True)
    for f in os.listdir(outdir):
        if f.startswith(name + ".") or f.startswith(name + "_"):
            os.remove(os.path.join(outdir, f))
    jobdir = os.path.join(LAB, "jobs", spec.get("group", "misc"), name)
    os.makedirs(jobdir, exist_ok=True)
    d, s = DEST[spec["dest"]], STREAM[spec["stream"]]
    lay = LAYOUT[spec["layout"]]
    kbps = spec.get("kbps", 0)
    stem_dir = stems_for(spec["rate"])
    # pass 1: XML without files, to get the cfg (which lists the channels in order)
    xml_path = os.path.join(jobdir, "job.xml")
    open(xml_path, "w", encoding="utf-8").write(build_xml(spec, outdir, name, []))
    if "cfg_template" in spec:  # reuse a cfg generated earlier (saves the JVM start) with text edits applied
        open(os.path.join(jobdir, "job.cfg"), "w").write(spec["cfg_template"])
        gen = type("G", (), {"stdout": "ok", "stderr": ""})()
    else:
      gen = subprocess.run([JAVA32, "-cp", ";".join([os.path.join(LAB, "java", "classes"), os.path.join(MAS, "DTSEncoder.jar"),
                                                     os.path.join(MAS, "lib", "*")]),
                            "com.dts.encoder.settings.GenCfg", xml_path, jobdir, "job", str(d), str(s), str(spec["rate"]),
                            str(lay), str(kbps)], cwd=MAS, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    cfg_path = os.path.join(jobdir, "job.cfg")
    if "ok" not in gen.stdout or not os.path.exists(cfg_path):
        return {"ok": False, "why": "cfg generation failed: " + gen.stdout[-300:] + gen.stderr[-300:], "spec": spec}
    cfg = open(cfg_path).read()
    # some configurators read these two from GUI panels, which are empty when run headless
    cfg = re.sub(r"BITWIDTH =[ \t]*\r?\n", "BITWIDTH = %d\n" % spec.get("bits", 24), cfg)
    cfg = re.sub(r"SAMPLERATE =[ \t]*\r?\n", "SAMPLERATE = %d\n" % spec["rate"], cfg)
    for line in spec.get("cfg_edit", []):  # (old, new) text replacements, for experiments
        cfg = cfg.replace(line[0], line[1])
    open(cfg_path, "w").write(cfg)
    files = channel_files(spec["layout"], cfg, stem_dir)
    open(xml_path, "w", encoding="utf-8").write(build_xml(spec, outdir, name, files))
    fps = spec.get("fps", "24")
    tc_end = spec.get("tc_end", "00:00:10:00")
    cmd = "E E  " + " ".join('"%s" ' % f for f in files) + " -r1 -s%s -t00:00:00:00 -u%s -v00:00:00:00 -w%s" % (fps, tc_end, tc_end)
    if spec.get("pa_att"):
        cmd += " -j%s -k%s -l%s" % (spec.get("pa_att_db", "6.0"), spec.get("fade_down", "2.0"), spec.get("fade_up", "2.0"))
    cmd += ' -f"%s"' % cfg_path
    cmd += spec.get("extra_cmd", "")
    cmd += ' -o"%s\\%s"' % (outdir, name)
    t0 = time.time()
    msgs = send(cmd)
    ok, log = wait_done(outdir, name, timeout)
    time.sleep(0.3)
    res = {"ok": ok, "log": log.strip()[-400:], "secs": round(time.time() - t0, 1), "cmd": cmd, "msgs": msgs,
           "out": [os.path.join(outdir, f) for f in sorted(os.listdir(outdir)) if f.startswith(name + ".") and not f.endswith("txt")],
           "cfg": cfg, "spec": spec}
    json.dump(res, open(os.path.join(jobdir, "result.json"), "w"), indent=1)
    return res


def run_many(specs, timeout=600, verbose=True):
    results = []
    for sp in specs:
        r = run_case(sp, timeout=timeout)
        Engine.drain()
        if verbose:
            print("%-40s %s %5.1fs %s" % (sp["name"], "OK  " if r["ok"] else "FAIL", r.get("secs", 0),
                                          "" if r["ok"] else (r.get("why") or r.get("log", ""))[:200].replace(chr(10), " | ")))
            sys.stdout.flush()
        results.append(r)
    return results


if __name__ == "__main__":
    specs = json.loads(sys.argv[1])
    run_many(specs if isinstance(specs, list) else [specs])
    Engine.close()
