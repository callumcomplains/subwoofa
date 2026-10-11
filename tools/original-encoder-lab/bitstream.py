"""The 13 Bitstream-tab reference encodes from docs/REFERENCE-ENCODES.md, plus extra variations."""
import sys
import lab

INFO_SHORT = "Subwoofa test ABC 123"
INFO_LONG = ("The quick brown fox jumps over the lazy dog. " * 34)[:1500]

SEC = dict(group="bitstream", dest="bd2", stream="express", rate=48000, layout="sa1.0", kbps=96)
MA51 = dict(group="bitstream", dest="bd", stream="ma", rate=48000, layout="5.1", kbps=0)
MA71 = dict(group="bitstream", dest="bd", stream="ma", rate=48000, layout="7.1w", kbps=0)


def case(base, name, **kw):
    d = dict(base)
    d.update(kw)
    d["name"] = name
    return d


SPECS = [
    case(SEC, "x1_default", pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0"),
    case(SEC, "x1b_no_att_flag"),
    case(SEC, "x2_att12", pa_att=True, pa_att_db="12.0", fade_down="0.0", fade_up="0.0"),
    case(SEC, "x3_att30_fades", pa_att=True, pa_att_db="30.0", fade_down="2.0", fade_up="4.0"),
    case(SEC, "x4_map", pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0",
         xml=dict(monoMetadataCValue="INF", monoMetadataLValue="0.0", monoMetadataRValue="6.0", monoMetadataLsValue="12.0",
                  monoMetadataRsValue="INF", monoMetadataLFEValue="INF")),
    case(SEC, "x5_dialog", pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0", dialog_mode=True),
    case(MA51, "p0_info_none"),
    case(MA51, "p1_info_short", programinfo=INFO_SHORT),
    case(MA51, "p2_info_long", programinfo=INFO_LONG),
    case(MA71, "w0_wide_off", wide=False),
    case(MA71, "w1_wide_on", wide=True),
    case(MA51, "s0_off"),
    case(MA51, "s1_single_clip", seamless_single=True, extra_cmd=" -y"),
]

if __name__ == "__main__":
    only = sys.argv[1:]
    specs = [s for s in SPECS if not only or any(o in s["name"] for o in only)]
    lab.run_many(specs, timeout=120)
    lab.Engine.close()
