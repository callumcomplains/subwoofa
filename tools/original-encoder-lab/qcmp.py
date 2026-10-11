"""qcmp.py group [filter]: size and SNR of the original's output against Subwoofa's for every case of a sweep group."""
import sys, os, json, glob
import numpy as np
import compare, lbrq

def summarize(res):
    rows = [r for r in res if r]
    if not rows: return None
    return dict(snr=np.mean([r[4] for r in rows]), cutoff=np.mean([r[3] for r in rows]), bands=np.mean([r[2] for r in rows], axis=0), n=len(rows))

if __name__ == "__main__":
    group = sys.argv[1]; flt = sys.argv[2] if len(sys.argv) > 2 else ""
    base = os.path.join(compare.LAB, "jobs", group)
    for name in sorted(os.listdir(base)):
        if flt not in name: continue
        f = os.path.join(base, name, "result.json")
        if not os.path.exists(f): continue
        r = json.load(open(f))
        if not r["ok"] or not r["out"]: continue
        spec = r["spec"]; orig = r["out"][0]
        nat = os.path.join(compare.LAB, "native", group, name + os.path.splitext(orig)[1])
        os.makedirs(os.path.dirname(nat), exist_ok=True)
        ok, msg = compare.native_encode(spec, nat)
        if not ok: print("%-36s native failed" % name); continue
        so = os.path.getsize(orig); sn = os.path.getsize(nat)
        a = summarize(lbrq.analyse(orig)); b = summarize(lbrq.analyse(nat))
        if not a or not b: print(name, "analysis failed"); continue
        print("%-36s size %7d / %7d   SNR %5.1f / %5.1f dB (orig/native)   bands orig %s  native %s   cutoff %5.0f / %5.0f" % (
            name, so, sn, a["snr"], b["snr"], " ".join("%4.1f" % v for v in a["bands"]), " ".join("%4.1f" % v for v in b["bands"]), a["cutoff"], b["cutoff"]), flush=True)
