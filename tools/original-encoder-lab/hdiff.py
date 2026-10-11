"""hdiff.py case [frame]: side-by-side EXSS header parse of the original's and Subwoofa's output for a sweep case."""
import sys, os, json, glob
import exssparse as E
case = sys.argv[1]; fr = int(sys.argv[2]) if len(sys.argv) > 2 else 0
grp = sys.argv[3] if len(sys.argv) > 3 else "sweep1_max"
def load(path):
    data = open(path, "rb").read()
    frames = E.find_frames(data)
    f, _ = E.parse_frame(data, frames[fr])
    return f
o = load(glob.glob("out/%s/%s.*" % (grp, case))[0] if not glob.glob("out/%s/%s.dtshd" % (grp, case)) else "out/%s/%s.dtshd" % (grp, case))
nfiles = glob.glob("native/%s/%s.*" % (grp, case))
n = load(nfiles[0])
def flat(f):
    d = {}
    for k, v in f.items():
        if k == "assets":
            for i, a in enumerate(v):
                for k2, v2 in a.items():
                    d["asset%d.%s" % (i, k2)] = v2
        else:
            d[k] = v
    return d
a, b = flat(o), flat(n)
for k in sorted(set(a) | set(b)):
    if a.get(k) != b.get(k):
        print("%-28s orig=%s\n%-28s native=%s" % (k, str(a.get(k))[:150], "", str(b.get(k))[:150]))
