"""Full parse of an extension-substream frame header (ETSI TS 102 114 layout, FFmpeg dca_exss.c as the guide).

exssparse.py file [frame#]   prints every field and the unparsed remainder of each asset descriptor
"""
import sys, struct

SYNC = bytes.fromhex("64582025")


class Bits:
    def __init__(self, data, pos=0):
        self.d, self.p = data, pos

    def get(self, n):
        v = 0
        for _ in range(n):
            v = (v << 1) | ((self.d[self.p >> 3] >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return v

    def bits(self, n):
        s = ""
        for _ in range(n):
            s += str((self.d[self.p >> 3] >> (7 - (self.p & 7))) & 1)
            self.p += 1
        return s


def find_frames(data):
    out, i = [], data.find(SYNC)
    while i >= 0:
        out.append(i)
        i = data.find(SYNC, i + 4)
    return out


def popcount(x):
    return bin(x).count("1")


def spk_count(mask):
    # channels per speaker-mask bit (a bit can stand for two speakers)
    two = [0x2 << 0]
    n = 0
    pairs = [0x8, 0x20, 0x80, 0x200, 0x800, 0x4000, 0x10000, 0x20000 if False else 0, 0x400, 0x100, 0x2000, 0x8000, 0x80000 if False else 0]
    return popcount(mask)  # filled in below if needed


# ETSI speaker activity mask bits: 0 C,1 LR,2 LsRs,3 LFE,4 Cs,5 LhRh,6 LsrRsr,7 Ch,8 Oh,9 LcRc,10 LwRw,11 LssRss,12 LFE2,13 LhcRhc,14 Chr,15 LhrRhr
PAIR_BITS = {1, 2, 5, 6, 9, 10, 11, 13, 15}


def count_chs(mask):
    return sum(2 if b in PAIR_BITS else 1 for b in range(16) if mask >> b & 1)


def parse_frame(data, off, verbose=True):
    b = Bits(data, off * 8)
    f = {}
    assert b.get(32) == 0x64582025
    f["user_defined"] = b.get(8)
    f["ss_index"] = b.get(2)
    wide = b.get(1)
    f["wide_header"] = wide
    f["header_size"] = b.get(8 + 4 * wide) + 1
    nsz = 16 + 4 * wide
    f["frame_size"] = b.get(nsz) + 1
    f["static_fields"] = b.get(1)
    nmixcfg = 0
    mixchs = []
    if f["static_fields"]:
        f["ref_clock"] = b.get(2)
        f["frame_dur_code"] = b.get(3)
        f["timestamp_present"] = b.get(1)
        if f["timestamp_present"]:
            f["timestamp"] = b.get(36)
        f["npresentations"] = b.get(3) + 1
        f["nassets"] = b.get(3) + 1
        f["active_exss_mask"] = [b.get(f["ss_index"] + 1) for _ in range(f["npresentations"])]
        f["active_asset_mask"] = [b.get(8) for m in f["active_exss_mask"] for _ in range(popcount(m))]
        f["mix_metadata"] = b.get(1)
        if f["mix_metadata"]:
            f["mix_adj_level"] = b.get(2)
            nb = (b.get(2) + 1) << 2
            f["mix_mask_bits"] = nb
            nmixcfg = b.get(2) + 1
            f["mix_out_masks"] = [b.get(nb) for _ in range(nmixcfg)]
            mixchs = [count_chs(m) for m in f["mix_out_masks"]]
    f["asset_sizes"] = [b.get(nsz) + 1 for _ in range(f.get("nassets", 1))]
    f["assets"] = []
    for _ in range(f.get("nassets", 1)):
        a = {}
        start = b.p
        a["descr_size"] = b.get(9) + 1
        a["asset_index"] = b.get(3)
        if f["static_fields"]:
            if b.get(1):
                a["type_descr"] = b.get(4)
            if b.get(1):
                a["language"] = b.get(24)
            if b.get(1):
                n = b.get(10) + 1
                a["text"] = bytes(b.get(8) for _ in range(n)).decode("latin-1")
            a["pcm_bits"] = b.get(5) + 1
            a["max_rate_code"] = b.get(4)
            a["nch"] = b.get(8) + 1
            a["one_to_one"] = b.get(1)
            if a["one_to_one"]:
                a["emb_stereo"] = b.get(1) if a["nch"] > 2 else 0
                a["emb_6ch"] = b.get(1) if a["nch"] > 6 else 0
                a["spk_mask_enabled"] = b.get(1)
                nb = 0
                if a["spk_mask_enabled"]:
                    nb = (b.get(2) + 1) << 2
                    a["spk_mask"] = b.get(nb)
                nsets = b.get(3)
                a["remap_sets"] = nsets
                masks = [b.get(nb) for _ in range(nsets)]
                a["remap_masks"] = masks
                a["remap"] = []
                for m in masks:
                    nch = b.get(5) + 1
                    rows = []
                    for _ in range(count_chs(m)):
                        rm = b.get(nch)
                        rows.append((rm, [b.get(5) for _ in range(popcount(rm))]))
                    a["remap"].append((nch, rows))
            else:
                a["repr_type"] = b.get(3)
        drc = b.get(1)
        a["drc_present"] = drc
        if drc:
            a["drc_code"] = b.get(8)
        if b.get(1):
            a["dialnorm"] = b.get(5)
        if drc and a.get("emb_stereo"):
            a["drc_stereo"] = b.get(8)
        if f.get("mix_metadata") and b.get(1):
            m = {}
            m["external_mix"] = b.get(1)
            m["post_gain_code"] = b.get(6)
            k = b.get(2)
            m["drc_ctl"] = k
            m["drc_val"] = b.get(8) if k == 3 else b.get(3)
            per_ch = b.get(1)
            m["per_ch_scale"] = per_ch
            if per_ch:
                m["scales"] = [[b.get(6) for _ in range(n)] for n in mixchs]
            else:
                m["scales"] = [b.get(6) for _ in range(nmixcfg)]
            ndm = a["nch"] + (6 if a.get("emb_6ch") else 0) + (2 if a.get("emb_stereo") else 0)
            m["matrix"] = []
            for n in mixchs:
                rows = []
                for _ in range(ndm):
                    mm = b.get(n)
                    rows.append((mm, [b.get(6) for _ in range(popcount(mm))]))
                m["matrix"].append(rows)
            a["mix"] = m
        a["coding_mode"] = b.get(2)
        if a["coding_mode"] == 0:
            a["ext_mask"] = b.get(12)
            em = a["ext_mask"]
            if em & 0x10:
                a["core_size"] = b.get(14) + 1
                if b.get(1):
                    a["core_sync_dist"] = b.get(2)
            if em & 0x20:
                a["xbr_size"] = b.get(14) + 1
            if em & 0x40:
                a["xxch_size"] = b.get(14) + 1
            if em & 0x80:
                a["x96_size"] = b.get(12) + 1
            if em & 0x100:
                a["lbr_size"] = b.get(14) + 1
                if b.get(1):
                    a["lbr_sync_dist"] = b.get(2)
        elif a["coding_mode"] == 2:
            a["lbr_size"] = b.get(14) + 1
            if b.get(1):
                a["lbr_sync_dist"] = b.get(2)
        end = start + a["descr_size"] * 8
        a["_rest_bits"] = end - b.p
        a["_rest"] = b.bits(max(0, end - b.p))
        b.p = end
        f["assets"].append(a)
    return f, b


def show(f):
    for k, v in f.items():
        if k == "assets":
            for i, a in enumerate(v):
                print("  asset", i)
                for k2, v2 in a.items():
                    print("     %-16s %s" % (k2, v2))
        else:
            print("  %-18s %s" % (k, v))


if __name__ == "__main__":
    data = open(sys.argv[1], "rb").read()
    frames = find_frames(data)
    n = int(sys.argv[2]) if len(sys.argv) > 2 else 0
    f, _ = parse_frame(data, frames[n])
    print(sys.argv[1], "frame", n, "at", frames[n], "of", len(frames))
    show(f)


def chain_frames(data, core_size=None):
    """Offsets of the extension substream frames of a stream that interleaves a core frame and an extension frame
    (find the first core sync, read its FSIZE, then walk core + extension)."""
    i = data.find(bytes.fromhex("7ffe8001"))
    out = []
    while i >= 0 and i + 16 < len(data):
        # core frame size from the header: bits 46..59 of the header after the sync (FSIZE is 14 bits at bit 46)
        b = Bits(data, (i + 4) * 8)
        b.get(1); b.get(5); b.get(1); b.get(7)  # FTYPE, SHORT, CPF, NBLKS(7)
        fsize = b.get(14) + 1
        j = i + fsize
        if data[j:j + 4] != SYNC:
            break
        f, _ = parse_frame(data, j)
        out.append(j)
        i = j + f["frame_size"]
        if data[i:i + 4] != bytes.fromhex("7ffe8001"):
            break
    return out
