"""Signal-to-noise of decoded streams against the 10-second source stems.

quality.py file...   -> per decoded channel: best-matching stem, lag, total SNR, SNR below / above half the sample rate
(the stems used are the ones at the stream's sample rate, found from the file name pattern or --rate)
"""
import os, subprocess, sys
import numpy as np

LAB = os.path.dirname(os.path.abspath(__file__))
STEMS = ["L", "R", "C", "LFE", "Lss", "Rss", "Lsr", "Rsr", "Cs"]


def decode(path, rate=None):
    args = ["ffmpeg", "-v", "error", "-i", path] + (["-ar", str(rate)] if rate else []) + ["-f", "s24le", "-"]
    out = subprocess.run(args, capture_output=True, stdin=subprocess.DEVNULL).stdout
    probe = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "stream=channels,sample_rate", "-of", "csv=p=0", path],
                           capture_output=True, text=True, stdin=subprocess.DEVNULL).stdout.strip().split(",")
    sr, nch = int(probe[0]), int(probe[1])
    b = np.frombuffer(out, np.uint8)
    n = len(b) // 3 // nch
    b = b[:n * nch * 3].reshape(-1, 3).astype(np.int32)
    v = b[:, 0] | (b[:, 1] << 8) | (b[:, 2] << 16)
    v = np.where(v >= 1 << 23, v - (1 << 24), v).astype(float)
    return sr, v.reshape(n, nch)


_stem_cache = {}


def stems_at(rate):
    if rate in _stem_cache:
        return _stem_cache[rate]
    d = os.path.join(LAB, "stems" if rate == 96000 else "stems%d" % rate)
    if not os.path.isdir(d):
        os.makedirs(d)
        for f in os.listdir(os.path.join(LAB, "stems")):
            subprocess.run(["ffmpeg", "-v", "error", "-y", "-i", os.path.join(LAB, "stems", f), "-ar", str(rate), "-c:a",
                            "pcm_s24le", os.path.join(d, f)], check=True)
    out = {}
    for n in STEMS:
        sr, v = decode(os.path.join(d, n + ".wav"))
        out[n] = v[:, 0]
    _stem_cache[rate] = out
    return out


def band(x, rate, lo, hi):
    X = np.fft.rfft(x)
    f = np.arange(len(X)) * (rate / 2) / (len(X) - 1)
    X[(f < lo) | (f >= hi)] = 0
    return np.fft.irfft(X, len(x))


def analyse(path, rate=None, verbose=True):
    sr, d = decode(path)
    src = stems_at(sr)
    res = []
    n = d.shape[1]
    for j in range(n):
        a = d[:, j]
        best = None
        cs = np.concatenate([[0.0], np.cumsum(a ** 2)]); e = cs[32768:] - cs[:-32768]
        st = int(np.argmax(e[sr // 4: len(e) - sr // 4])) + sr // 4
        w = slice(st, st + 32768)
        for nm, b in src.items():
            # lag search around 0 (the codec delay): cross-correlation of a window
            L = 8192
            seg = a[w]
            c = np.fft.irfft(np.fft.rfft(seg, 65536 * 2) * np.conj(np.fft.rfft(b[w], 65536 * 2)))
            c = np.concatenate([c[-L:], c[:L]])
            lag = int(np.argmax(c)) - L
            pk = c.max() / np.sqrt((seg ** 2).sum() * (b[w] ** 2).sum() + 1e-9)
            if best is None or pk > best[0]:
                best = (pk, nm, lag)
        pk, nm, lag = best
        if pk < 0.3:
            res.append((j, None, 0, None, None, None))
            continue
        b = src[nm]
        m = min(len(b) - abs(lag), len(a) - abs(lag)) - 2000
        aa = a[lag:lag + m] if lag >= 0 else a[:m]
        bb = b[:m] if lag >= 0 else b[-lag:-lag + m]
        aa, bb = aa[1000:-1000], bb[1000:-1000]
        e = aa - bb
        tot = 10 * np.log10((bb ** 2).sum() / max((e ** 2).sum(), 1e-9))
        lo = band(bb, sr, 0, sr / 4), band(e, sr, 0, sr / 4)
        hi = band(bb, sr, sr / 4, sr / 2), band(e, sr, sr / 4, sr / 2)
        slo = 10 * np.log10((lo[0] ** 2).sum() / max((lo[1] ** 2).sum(), 1e-9))
        shi = 10 * np.log10((hi[0] ** 2).sum() / max((hi[1] ** 2).sum(), 1e-9)) if (hi[0] ** 2).sum() > 0 else float("nan")
        res.append((j, nm, lag, tot, slo, shi))
    if verbose:
        print(os.path.basename(path), "%d Hz" % sr)
        for r in res:
            if r[1] is None:
                print("   ch%d  unmatched/silent" % r[0])
            else:
                print("   ch%d ~ %-3s lag %+5d  total %6.1f  low %6.1f  high %6.1f dB" % r)
    return res


if __name__ == "__main__":
    for f in sys.argv[1:]:
        analyse(f)
