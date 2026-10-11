"""Sweep 1: every legal combination at its highest bit rate, through the original encoder."""
import sys, json
import lab

DEST_N = {"BD_PRIMARY": "bd", "BD_SECONDARY": "bd2", "DVD": "dvd", "DTS_CD": "cd", "DECE": "dece"}
STREAM_N = {"EXPRESS": "express", "DIGITAL_SURROUND": "core", "DIGITAL_SURROUND_9624": "core96",
            "DIGITAL_SURROUND_ES": "es", "HI_RES": "hra", "MASTER_AUDIO": "ma"}
LAYOUT_BY_ENUM = {  # enum name suffix -> short id used in lab.py
    "PA_10_C": "1.0", "PA_11_C_LFE": "1.1", "PA_20_Lt_Rt": "2.0lt", "PA_20_Lo_Ro": "2.0", "PA_21_Lt_Rt_LFE": "2.1lt",
    "PA_21_L_R_LFE": "2.1", "PA_30_L_R_S": "3.0s", "PA_30_L_C_R": "3.0", "PA_31_L_R_S_LFE": "3.1s", "PA_31_L_C_R_LFE": "3.1",
    "PA_40_L_C_R_S": "4.0cs", "PA_40_L_R_Ls_Rs": "4.0", "PA_41_L_C_R_S_LFE": "4.1cs", "PA_41_L_R_Ls_Rs_LFE": "4.1",
    "PA_50_L_R_C_Ls_Rs": "5.0", "PA_51_L_R_C_LFE_Ls_Rs": "5.1",
    "PA_60_ES_Matrix_L_R_C_Ls_Rs_Cs": "6.0m", "PA_61_ES_Matrix_L_R_C_LFE_Ls_Rs_Cs": "6.1m",
    "PA_60_ES_Discrete_L_R_C_Ls_Rs_Cs": "6.0d", "PA_61_ES_Discrete_L_R_C_LFE_Ls_Rs_Cs": "6.1d",
    "PA_70_L_R_C_Lss_Rss_Lsr_Rsr": "7.0", "PA_71_L_R_C_LFE_Ls_Rs_Lw_Rw": "7.1w", "PA_71_L_R_C_LFE_Ls_Rs_Cs_Oh": "7.1oh",
    "PA_71_L_R_C_LFE_Ls_Rs_Cs_Ch": "7.1ch", "PA_71_L_R_C_LFE_Ls_Rs_Lsr_Rsr": "7.1sr",
    "PA_71_L_R_C_LFE_Ls_Rs_Lhs_Rhs": "7.1hs", "PA_71_L_R_C_LFE_Ls_Rs_Lh_Rh": "7.1h",
    "PA_71_L_R_C_LFE_Lss_Rss_Lsr_Rsr": "7.1",
    "SA_10_C": "sa1.0", "SA_20_Lo_Ro": "sa2.0", "SA_51_L_R_C_LFE_Ls_Rs": "sa5.1",
}


def rows(path="legal_all.tsv"):
    for line in open(path):
        d, s, r, l, b = line.rstrip("\n").split("\t")
        yield dict(dest=DEST_N[d], stream=STREAM_N[s], rate=int(r), layout=LAYOUT_BY_ENUM[l],
                   rates=[int(x) for x in b.split(",")])


if __name__ == "__main__":
    pick = sys.argv[1] if len(sys.argv) > 1 else "max"
    specs = []
    for r in rows():
        for kb in ([max(r["rates"])] if pick == "max" else [min(r["rates"])] if pick == "min" else r["rates"]):
            specs.append(dict(name="%s_%s_%d_%s_%d" % (r["dest"], r["stream"], r["rate"], r["layout"], kb), group="sweep1_" + pick,
                              dest=r["dest"], stream=r["stream"], rate=r["rate"], layout=r["layout"], kbps=kb))
    print(len(specs), "cases")
    lab.run_many(specs, timeout=300)
    lab.Engine.close()
