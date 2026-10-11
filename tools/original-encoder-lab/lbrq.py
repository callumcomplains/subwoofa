"""Per-band SNR and spectral cutoff of decoded Express streams against the source stems."""
import sys, os, glob, subprocess
import numpy as np
import quality as Q

def bandsnr(a, b, sr, edges):
    X = np.fft.rfft(b); Y = np.fft.rfft(a)
    f = np.arange(len(X)) * (sr / 2) / (len(X) - 1)
    out = []
    for lo, hi in zip(edges[:-1], edges[1:]):
        m = (f >= lo) & (f < hi)
        s = (np.abs(X[m]) ** 2).sum(); e = (np.abs(Y[m] - X[m]) ** 2).sum()
        out.append(10 * np.log10(s / max(e, 1e-9)) if s > 0 else float("nan"))
    return out

def cutoff(a, sr):
    X = np.abs(np.fft.rfft(a * np.hanning(len(a)))) ** 2
    f = np.arange(len(X)) * (sr / 2) / (len(X) - 1)
    # smooth in 200 Hz bins
    step = max(1, int(200 / (f[1])))
    p = np.add.reduceat(X, np.arange(0, len(X), step))
    ff = f[::step]
    db = 10 * np.log10(p / p.max() + 1e-20)
    idx = np.where(db > -70)[0]
    return ff[idx.max()] if len(idx) else 0

def analyse(path):
    sr, d = Q.decode(path)
    src = Q.stems_at(sr)
    res = []
    for j in range(d.shape[1]):
        a = d[:, j]
        best = None
        for nm, b in src.items():
            cs = np.concatenate([[0.0], np.cumsum(a ** 2)]); e = cs[32768:] - cs[:-32768]
            st = int(np.argmax(e[sr // 4: len(e) - sr // 4])) + sr // 4
            w = slice(st, st + 32768)
            L = 8192
            c = np.fft.irfft(np.fft.rfft(a[w], 131072) * np.conj(np.fft.rfft(b[w], 131072)))
            c = np.concatenate([c[-L:], c[:L]])
            lag = int(np.argmax(c)) - L
            pk = c.max() / np.sqrt((a[w] ** 2).sum() * (b[w] ** 2).sum() + 1e-9)
            if best is None or pk > best[0]: best = (pk, nm, lag)
        pk, nm, lag = best
        if pk < 0.3: res.append(None); continue
        b = src[nm]; m = min(len(a), len(b)) - abs(lag) - 4000
        aa = a[lag + 2000: lag + 2000 + m] if lag >= 0 else a[2000: 2000 + m]
        bb = b[2000: 2000 + m] if lag >= 0 else b[2000 - lag: 2000 - lag + m]
        res.append((nm, lag, bandsnr(aa, bb, sr, [0, 4000, 8000, 12000, 16000, 24000]), cutoff(aa, sr), 10 * np.log10((bb ** 2).sum() / max(((aa - bb) ** 2).sum(), 1e-9))))
    return res

if __name__ == "__main__":
    for p in sys.argv[1:]:
        r = analyse(p)
        print(os.path.basename(p))
        for j, x in enumerate(r):
            if x is None: print("   ch%d unmatched" % j); continue
            nm, lag, bs, co, tot = x
            print("   ch%d ~%-3s lag %+5d  total %5.1f dB   bands(0-4,4-8,8-12,12-16,16-24k) %s  cutoff %5.0f Hz" % (j, nm, lag, tot, " ".join("%5.1f" % v for v in bs), co))
