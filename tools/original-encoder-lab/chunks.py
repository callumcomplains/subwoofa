"""chunks.py file...: list the DTS-HD container chunks (8-byte name, 8-byte big-endian size)."""
import sys, struct
def chunks(path):
    d = open(path, "rb").read()
    i, out = 0, []
    while i + 16 <= len(d):
        name = d[i:i+8]; size = struct.unpack(">Q", d[i+8:i+16])[0]
        out.append((name.decode("latin-1"), i, size, d[i+16:i+16+min(size, 160)]))
        i += 16 + size
        if not name.strip(b"\0").isalnum() and not name.startswith(b"DTSHDHDR"):
            if not all(32 <= c < 127 for c in name): break
    return out
if __name__ == "__main__":
    for f in sys.argv[1:]:
        print(f)
        for n, o, s, body in chunks(f):
            print("  %-9s @%-9d size %-8d %s" % (n, o, s, body[:48].hex() if n != "FILEINFO" else body))
