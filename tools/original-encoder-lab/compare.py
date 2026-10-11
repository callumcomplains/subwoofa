"""Encode the same case with Subwoofa's native encoder and compare it with the original's output.

compare.py <sweepdir> [filter]   e.g.  compare.py sweep1_max bd_hra
"""
import json, os, re, subprocess, sys

LAB = os.path.dirname(os.path.abspath(__file__))
SW = os.environ.get("SW_BIN", r"E:\FormatProgram\subwoofa\target\release\subwoofa.exe")

REAR = os.environ.get("LAB_REAR", "")
STREAM_ID = {"express": "dts-express", "core": "dts", "core96": "dts-96-24", "es": "dts-es", "hra": "dtshd-hra", "ma": "dtshd-ma"}
DEST_ID = {"bd": "bluray-primary", "bd2": "bluray-secondary", "dvd": "dvd", "cd": "dts-music-disc", "dece": "digital-delivery"}
LAYOUT_ID = {"1.0": "1.0", "1.1": "1.1", "2.0lt": "2.0-ltrt", "2.0": "2.0", "2.1": "2.1", "3.0s": "3.0-lrs", "3.0": "3.0",
             "3.1s": "3.1-lrs", "3.1": "3.1", "4.0cs": "4.0-lcrs", "4.0": "4.0-quad", "4.1cs": "4.1-lcrs", "4.1": "4.1-quad",
             "5.0": "5.0", "5.1": "5.1", "6.0m": "6.0-matrix", "6.1m": "6.1-matrix", "6.0d": "6.0-discrete",
             "6.1d": "6.1-discrete", "7.0": "7.0-side-rear", "7.1w": "7.1-wide", "7.1oh": "7.1-cs-oh", "7.1ch": "7.1-cs-ch",
             "7.1sr": "7.1-ls-rs-lsr-rsr", "7.1hs": "7.1-height-side", "7.1h": "7.1-height", "7.1": "7.1-side-rear",
             "sa1.0": "1.0", "sa2.0": "2.0", "sa5.1": "5.1"}
LAYOUT_CH = {  # channel names Subwoofa wants for each layout id
    "1.0": "C", "1.1": "C LFE", "2.0-ltrt": "Lt Rt", "2.0": "L R", "2.1": "L R LFE", "3.0-lrs": "L R S", "3.0": "L C R",
    "3.1-lrs": "L R S LFE", "3.1": "L C R LFE", "4.0-lcrs": "L C R S", "4.0-quad": "L R Ls Rs", "4.1-lcrs": "L C R S LFE",
    "4.1-quad": "L R Ls Rs LFE", "6.0-matrix-premixed": "L R C Ls Rs", "6.1-matrix-premixed": "L R C LFE Ls Rs", "5.0": "L R C Ls Rs", "5.1": "L R C LFE Ls Rs", "6.0-matrix": "L R C Ls Rs Cs",
    "6.1-matrix": "L R C LFE Ls Rs Cs", "6.0-discrete": "L R C Ls Rs Cs", "6.1-discrete": "L R C LFE Ls Rs Cs",
    "7.0-side-rear": "L R C Lss Rss Lsr Rsr", "7.1-wide": "L R C LFE Ls Rs Lw Rw", "7.1-cs-oh": "L R C LFE Ls Rs Cs Oh",
    "7.1-cs-ch": "L R C LFE Ls Rs Cs Ch", "7.1-ls-rs-lsr-rsr": "L R C LFE Ls Rs Lsr Rsr",
    "7.1-height-side": "L R C LFE Ls Rs Lhs Rhs", "7.1-height": "L R C LFE Ls Rs Lh Rh", "7.1-side-rear": "L R C LFE Lss Rss Lsr Rsr",
}
STEM = {"L": "L", "R": "R", "C": "C", "LFE": "LFE", "Ls": "Lss", "Rs": "Rss", "Lss": "Lss", "Rss": "Rss", "Lsr": "Lsr" + REAR,
        "Rsr": "Rsr" + REAR, "Cs": "Cs", "S": "Cs", "Lw": "Lsr", "Rw": "Rsr", "Lh": "Lsr", "Rh": "Rsr", "Lhs": "Lsr", "Rhs": "Rsr",
        "Oh": "Rsr", "Ch": "Rsr", "Lt": "L", "Rt": "R"}



def stems_dir(rate):
    return os.path.join(LAB, "stems" if rate == 96000 else "stems%d" % rate)


def native_encode(spec, outpath, extra=()):
    lid = LAYOUT_ID[spec["layout"]]
    if spec["stream"] == "ma" and spec["layout"] in ("6.0m", "6.1m"):
        lid += "-premixed"  # lossless streams take the already matrixed 5.x channels
    args = [SW, "encode", "--stream", STREAM_ID[spec["stream"]], "--layout", lid, "--dest", DEST_ID[spec["dest"]],
            "--sample-rate", str(spec["rate"]), "--bitrate", str(spec["kbps"]), "-o", outpath, "--engine", "native"]
    for ch in LAYOUT_CH[lid].split():
        args += ["--input", "%s=%s" % (ch, os.path.join(stems_dir(spec["rate"]), STEM[ch] + ".wav"))]
    args += list(extra)
    r = subprocess.run(args, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return r.returncode == 0, (r.stdout + r.stderr)[-600:]


def verify_text(path, deep=True):
    args = [SW, "verify"] + (["--deep"] if deep else []) + [path]
    r = subprocess.run(args, capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return r.stdout + r.stderr


def sections(text):
    """verify output -> {section: {key: value}}"""
    out, cur = {}, None
    for line in text.splitlines():
        if line and not line.startswith(" ") and not line.startswith("PASSED") and not line.startswith("FAILED"):
            cur = line.strip()
            out[cur] = {}
        elif cur is not None and line.startswith("  ") and line.strip():
            m = re.match(r"\s{2}(.*?)\s{2,}(.*)$", line)
            if m:
                out[cur][m.group(1)] = m.group(2)
    return out


SKIP_XLL = ("asset size", "components in this substream", "substream size")
SKIP = {"Decoded PCM (24-bit) MD5", "Duration", "Access units", "Programme length (container)", "Codec delay (container)",
        "TIMECODE", "BUILDVER", "Silence before the first sound", "Silence after the last sound"}


def diff(a, b, wanted=("Core substream header (first frame)", "Extension substream header (first frame)")):
    out = []
    for sec in wanted:
        sa, sb = a.get(sec, {}), b.get(sec, {})
        for k in sorted(set(sa) | set(sb)):
            if k in SKIP or k.startswith("channel ") and sec == "Decoded audio":
                continue
            if k in SKIP_XLL and "XLL" in str(sa.get("components in this substream", "")) and "XLL" in str(sb.get("components in this substream", "")):
                continue  # sizes of lossless data depend on the audio coding, not on the layout of the stream
            if sa.get(k) != sb.get(k):
                out.append("%s | %s: orig=%s native=%s" % (sec.split(" (")[0], k, sa.get(k), sb.get(k)))
    return out


def top(text):
    """The leading summary block as a dict."""
    d = {}
    for line in text.splitlines()[2:]:
        if not line.strip():
            break
        m = re.match(r"\s{2}(.*?)\s{2,}(.*)$", line)
        if m:
            d[m.group(1)] = m.group(2)
    return d


def compare_case(res_json):
    r = json.load(open(res_json))
    spec = r["spec"]
    if not r["ok"] or not r["out"]:
        return spec, ["original failed: " + (r.get("log") or r.get("why", ""))[:100]]
    orig = r["out"][0]
    ext = os.path.splitext(orig)[1]
    nat_dir = os.path.join(LAB, "native", spec.get("group", "misc"))
    os.makedirs(nat_dir, exist_ok=True)
    nat = os.path.join(nat_dir, spec["name"] + (".dtshd" if ext == ".dtshd" else ext))
    ok, msg = native_encode(spec, nat)
    if not ok or not os.path.exists(nat):
        return spec, ["native failed: " + msg.strip().splitlines()[-1][:150] if msg.strip() else "native failed"]
    to, tn = verify_text(orig), verify_text(nat)
    so, sn = sections(to), sections(tn)
    d = []
    topo, topn = top(to), top(tn)
    for k in ("Format", "Sample rate", "Core channels", "Extension channels", "Average bit rate"):
        if topo.get(k) != topn.get(k):
            d.append("Summary | %s: orig=%s native=%s" % (k, topo.get(k), topn.get(k)))
    d += diff(so, sn)
    return spec, d


if __name__ == "__main__":
    group = sys.argv[1]
    flt = sys.argv[2] if len(sys.argv) > 2 else ""
    base = os.path.join(LAB, "jobs", group)
    total = same = 0
    for name in sorted(os.listdir(base)):
        if flt not in name:
            continue
        f = os.path.join(base, name, "result.json")
        if not os.path.exists(f):
            continue
        spec, d = compare_case(f)
        total += 1
        if not d:
            same += 1
            print("%-44s identical headers" % name)
        else:
            print("%-44s %d differences" % (name, len(d)))
            for x in d[:14]:
                print("      " + x)
    print("%d cases, %d with identical headers" % (total, same))
