import sys, numpy as np, quality as Q, lbrq
def levels(path, edges):
    sr, d = Q.decode(path); src = Q.stems_at(sr)
    a = d[:, 0]
    best=None
    for nm,b in src.items():
        cs = np.concatenate([[0.0], np.cumsum(a ** 2)]); e = cs[32768:] - cs[:-32768]
        st = int(np.argmax(e[sr // 4: len(e) - sr // 4])) + sr // 4
        w = slice(st, st + 32768); L=8192
        c = np.fft.irfft(np.fft.rfft(a[w], 131072) * np.conj(np.fft.rfft(b[w], 131072)))
        c = np.concatenate([c[-L:], c[:L]]); lag = int(np.argmax(c)) - L
        pk = c.max() / np.sqrt((a[w] ** 2).sum() * (b[w] ** 2).sum() + 1e-9)
        if best is None or pk > best[0]: best = (pk, nm, lag)
    pk, nm, lag = best; b = src[nm]
    m = min(len(a), len(b)) - abs(lag) - 4000
    aa = a[lag + 2000: lag + 2000 + m] if lag >= 0 else a[2000: 2000 + m]
    bb = b[2000: 2000 + m] if lag >= 0 else b[2000 - lag: 2000 - lag + m]
    A = np.abs(np.fft.rfft(aa)) ** 2; B = np.abs(np.fft.rfft(bb)) ** 2
    f = np.arange(len(A)) * (sr / 2) / (len(A) - 1)
    out=[]
    for lo,hi in zip(edges[:-1],edges[1:]):
        mk=(f>=lo)&(f<hi); out.append(10*np.log10(A[mk].sum()/max(B[mk].sum(),1e-12)))
    return nm, out
edges=[0,1000,2000,4000,6000,8000,10000,12000,14000,16000,18000,20000,24000]
for p in sys.argv[1:]:
    nm,l=levels(p,edges); print(p.split('/')[-1], nm); print("  ", " ".join("%5.1f"%x for x in l))
print("edges", edges)
