"""aus.py: split a .dtshd STRMDATA into access units using the NAVI-TBL (3-byte cumulative offsets after a 7-byte header)."""
import struct, chunks
def access_units(path):
    d = open(path, "rb").read()
    strm = nav = None
    for n, o, s, b in chunks.chunks(path):
        if n == "STRMDATA": strm = d[o+16:o+16+s]
        if n == "NAVI-TBL": nav = d[o+16:o+16+s]
    cnt = struct.unpack(">I", nav[:4])[0]
    # header: 4 bytes count, then 2 bytes version?, 1 byte entry width?, then 4 bytes ... entries
    # entries are 3-byte big-endian cumulative sizes
    ents = nav[4:]
    # find alignment: first entries 00 01 03 00 00 00 00 06 44 ...: skip until a plausible monotonic sequence
    best = None
    for skip in range(0, 12):
        vals = [int.from_bytes(ents[skip+3*i: skip+3*i+3], "big") for i in range(cnt)]
        if all(vals[i] < vals[i+1] for i in range(len(vals)-1)) and vals[-1] <= len(strm):
            best = vals; break
    return strm, best
if __name__ == "__main__":
    import sys
    s, v = access_units(sys.argv[1])
    print(len(s), len(v) if v else None, v[:6] if v else None, v[-3:] if v else None)
