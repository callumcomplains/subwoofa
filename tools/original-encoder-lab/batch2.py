import json, lab
MA51 = dict(group="bitstream2", dest="bd", stream="ma", rate=48000, layout="5.1", kbps=0)
SEC = dict(group="bitstream2", dest="bd2", stream="express", rate=48000, layout="sa1.0", kbps=96)
def case(base, name, **kw):
    d = dict(base); d.update(kw); d["name"] = name; return d
CSV = r"C:\Users\Callum\AppData\Local\Temp\claude\E--FormatProgram\cccb6f64-f44b-4c8d-85e6-cc21df66cdda\scratchpad\lab\csv"
specs = [
    case(MA51, "s2_csv_two", xml={"seamlessbranching-csvbranchpoints": True, "seamlessbranching-csvfilename": CSV + r"\two.csv"}, extra_cmd=' -y"%s\two.csv"' % CSV),
    case(MA51, "s3_csv_one", xml={"seamlessbranching-csvbranchpoints": True, "seamlessbranching-csvfilename": CSV + r"\one.csv"}, extra_cmd=' -y"%s\one.csv"' % CSV),
    case(MA51, "s4_csv_edge", xml={"seamlessbranching-csvbranchpoints": True, "seamlessbranching-csvfilename": CSV + r"\edge.csv"}, extra_cmd=' -y"%s\edge.csv"' % CSV),
    case(MA51, "p3_info_utf8", programinfo="Café ünï 日本 \u00a9 test"),
    case(MA51, "p4_info_lines", programinfo="line one\nline two\n\nline four"),
    dict(SEC, name="d_bd2_1_0_dialog", dialog_mode=True, pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0"),
    dict(SEC, name="d_bd2_2_0_dialog", layout="sa2.0", kbps=128, dialog_mode=True, pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0"),
    dict(SEC, name="d_bd2_5_1_dialog", layout="sa5.1", kbps=256, dialog_mode=True, pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0"),
    dict(SEC, name="d_bd2_5_1_nodialog", layout="sa5.1", kbps=256, pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0"),
    dict(SEC, dest="dece", name="d_dece_1_0_dialog", kbps=96, dialog_mode=True),
    dict(SEC, dest="dece", name="d_dece_5_1_dialog", layout="sa5.1", kbps=384, dialog_mode=True),
]
# mapping resolution test: C gain values
for v in ["0.3", "0.4", "14.8", "14.9", "15.4", "15.6", "29.4", "29.6", "30.9", "31.1", "59.0", "70.0"]:
    specs.append(dict(SEC, name="m_C_%s" % v.replace(".", "p"), pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0",
                      xml=dict(monoMetadataCValue=v, monoMetadataLValue="INF", monoMetadataRValue="INF", monoMetadataLsValue="INF", monoMetadataRsValue="INF", monoMetadataLFEValue="INF")))
res = lab.run_many(specs, timeout=120)
lab.Engine.close()
