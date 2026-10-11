"""Reproduce the Bitstream-tab reference encodes with Subwoofa and compare the metadata with the original's."""
import os, subprocess, sys, json
import exssparse as E, chunks, compare

LAB = os.path.dirname(os.path.abspath(__file__))
SW = compare.SW
S48 = os.path.join(LAB, "stems48000")
S96 = os.path.join(LAB, "stems")
def stem(d, n): return os.path.join(d, n + ".wav")

def enc(args, out):
    os.makedirs(os.path.dirname(out), exist_ok=True)
    r = subprocess.run([SW, "encode"] + args + ["-o", out], capture_output=True, text=True, stdin=subprocess.DEVNULL)
    return r.returncode == 0, (r.stdout + r.stderr)[-300:]

sec1 = ["--stream", "dts-express", "--layout", "1.0", "--dest", "bluray-secondary", "--sample-rate", "48000", "--bitrate", "96", "--input", "C=" + stem(S48, "C")]
ma51 = ["--stream", "dtshd-ma", "--layout", "5.1", "--dest", "bluray-primary", "--sample-rate", "48000", "--bitrate", "0"] + sum([["--input", "%s=%s" % (c, stem(S48, s))] for c, s in (("L", "L"), ("R", "R"), ("C", "C"), ("LFE", "LFE"), ("Ls", "Lss"), ("Rs", "Rss"))], [])
ma71w = ["--stream", "dtshd-ma", "--layout", "7.1-wide", "--dest", "bluray-primary", "--sample-rate", "48000", "--bitrate", "0"] + sum([["--input", "%s=%s" % (c, stem(S48, s))] for c, s in (("L", "L"), ("R", "R"), ("C", "C"), ("LFE", "LFE"), ("Ls", "Lss"), ("Rs", "Rss"), ("Lw", "Lsr"), ("Rw", "Rsr"))], [])
CASES = {
    "x1_default": (sec1 + ["--primary-attenuation", "6"], "bitstream"),
    "x2_att12": (sec1 + ["--primary-attenuation", "12"], "bitstream"),
    "x3_att30_fades": (sec1 + ["--primary-attenuation", "30", "--fade-down", "2", "--fade-up", "4"], "bitstream"),
    "x4_map": (sec1 + ["--primary-attenuation", "6", "--mono-map", "L=0,R=6,Ls=12"], "bitstream"),
    "x5_dialog": (sec1 + ["--primary-attenuation", "6", "--dialog-mode"], "bitstream"),
    "p0_info_none": (ma51, "bitstream"),
    "p1_info_short": (ma51 + ["--program-info", "Subwoofa test ABC 123"], "bitstream"),
    "w0_wide_off": (ma71w, "bitstream"),
    "w1_wide_on": (ma71w + ["--wide-remap"], "bitstream"),
    "s1_single_clip": (ma51 + ["--seamless", "single"], "bitstream"),
}

def exss_meta(path):
    d = open(path, "rb").read(); fr = E.find_frames(d)
    out = []
    for o in fr:
        f, _ = E.parse_frame(d, o)
        a = f["assets"][0]
        out.append((f["header_size"], f["frame_size"], a.get("mix"), a.get("remap"), a.get("remap_masks"), a.get("_rest"), a.get("descr_size"), a.get("emb_6ch"), a.get("one_to_one")))
    return out

if __name__ == "__main__":
    only = sys.argv[1:]
    for name, (args, grp) in CASES.items():
        if only and name not in only: continue
        orig = os.path.join(LAB, "out", grp, name + ".dtshd")
        nat = os.path.join(LAB, "native", "bs", name + ".dtshd")
        ok, msg = enc(args, nat)
        if not ok: print(name, "NATIVE FAILED", msg); continue
        res = []
        if name.startswith(("x", "w")):
            mo, mn = exss_meta(orig), exss_meta(nat)
            n = min(len(mo), len(mn))
            diff = [i for i in range(n) if mo[i] != mn[i]]
            res.append("frames %d/%d, EXSS metadata differs in %d frames" % (len(mo), len(mn), len(diff)))
            for i in diff[:2]:
                res.append("   frame %d orig %s" % (i, mo[i])); res.append("   frame %d nat  %s" % (i, mn[i]))
        co = {c[0]: (c[2], bytes(c[3])) for c in chunks.chunks(orig)}
        cn = {c[0]: (c[2], bytes(c[3])) for c in chunks.chunks(nat)}
        for k in ("FILEINFO", "BRANCHPT"):
            if (k in co) != (k in cn) or (k in co and co[k] != cn[k]):
                res.append("   chunk %s orig %s native %s" % (k, co.get(k), cn.get(k)))
        print("%-18s %s" % (name, "OK" if len(res) == 0 or (len(res) == 1 and "differs in 0 frames" in res[0]) else ""))
        for r in res: print("   " + r)
