"""bindiff.py a b: summary of where two files differ."""
import sys
a, b = open(sys.argv[1], "rb").read(), open(sys.argv[2], "rb").read()
print("sizes", len(a), len(b))
n = min(len(a), len(b))
runs = []
i = 0
while i < n:
    if a[i] != b[i]:
        j = i
        while j < n and (a[j] != b[j] or (j + 1 < n and a[j + 1] != b[j + 1])):
            j += 1
        runs.append((i, j - i))
        i = j
    else:
        i += 1
print(len(runs), "differing runs;", sum(r[1] for r in runs), "bytes")
for off, ln in runs[:int(sys.argv[3]) if len(sys.argv) > 3 else 12]:
    print("  @%d len %d  a=%s b=%s" % (off, ln, a[off:off + min(ln, 24)].hex(), b[off:off + min(ln, 24)].hex()))
