import json, math
import numpy as np

def level_db(c):
    if c >= 31: return (61 - c) * 0.5
    if c >= 16: return 15 + (31 - c)
    return 30 + (16 - c) * 2

LEVELS = {c: level_db(c) for c in range(1, 62)}

def quant(db, tie="up"):
    best = None
    for c, l in LEVELS.items():
        e = abs(l - db)
        if best is None or e < best[0] - 1e-9 or (abs(e - best[0]) < 1e-9 and ((tie == "up" and c < best[1]) or (tie == "down" and c > best[1]))):
            best = (e, c)
    return best[1]

FL = 4096 / 48000

def model(A, Td, Tu, nframes, t_end, off=0.0, tie="up", shape="sqrtinv"):
    M = 10 ** (A / 40)
    out = []
    for k in range(nframes):
        t = k * FL + off
        u = 1.0
        if Td > 0 and t < Td:
            u = min(u, t / Td)
        if Tu > 0 and t > t_end - Tu:
            u = min(u, max(0.0, (t_end - t) / Tu))
        if Td <= 0 and Tu <= 0:
            u = 1.0
        u = max(0.0, min(1.0, u))
        db = 40 * math.log10(1 + (M - 1) * u)
        out.append(quant(min(db, A), tie))
    return out

if __name__ == "__main__":
    d = json.load(open("fade_codes.json"))
    for off in (0.0, FL / 2, FL, -FL / 2):
        for tie in ("up", "down"):
            tot = 0; bad = 0
            for name, v in d.items():
                _, A, T = name.split("_")
                A = float(A.replace("p", ".")); T = float(T.replace("p", "."))
                m = model(A, T, 0.0, len(v), 10.0, off, tie)
                diff = sum(1 for a, b in zip(m, v) if a != b)
                tot += len(v); bad += diff
            print("off %+.3f tie %s: %d of %d codes differ" % (off, tie, bad, tot))
