import exssparse as E, glob, os, collections, sys
def pad_info(path, k=5):
    d = open(path, "rb").read(); fr = E.find_frames(d)
    if len(fr) <= k: return None
    off = fr[k]
    f, b = E.parse_frame(d, off)
    # after assets: bc core flags
    start = off * 8
    used_before_bc = b.p - start
    bc = []
    for _ in range(f["nassets"] if "nassets" in f else 1):
        x = b.get(1); bc.append(x)
        if x: b.get(5)
    used = b.p - start
    total = f["header_size"] * 8
    return dict(hs=f["header_size"], used=used, pad=total - 16 - used, bc=bc, nassets=len(f["assets"]))
if __name__ == "__main__":
    rows = collections.defaultdict(list)
    for p in sorted(glob.glob("out/sweep1_max/*.dtshd")):
        n = os.path.basename(p)[:-6]
        o = pad_info(p)
        q = "native/sweep1_max/" + n + ".dtshd"
        nn = pad_info(q) if os.path.exists(q) else None
        if o is None: continue
        rows[(o["hs"], o["pad"], tuple(o["bc"]))].append((n, nn and (nn["hs"], nn["pad"])))
    for k, v in sorted(rows.items()):
        print("orig header %d  pad_bits %d bc %s : %d cases; native sample %s" % (k[0], k[1], k[2], len(v), [x[1] for x in v[:6]]))
        print("       ", [x[0] for x in v[:5]])
