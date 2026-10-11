import lab, exssparse as E
SEC = dict(group="bitstream4", dest="bd2", stream="express", rate=48000, layout="sa1.0", kbps=96)
specs = [
 dict(SEC, name="u_flag_off_12", pa_att=False, xml=dict(paMetadataPrimaryValue="12.0", paMetadataFadeDownValue="2.0", paMetadataFadeUpValue="2.0")),
 dict(SEC, name="u_flag_on_12_noFades", pa_att=True, pa_att_db="12.0", fade_down="0.0", fade_up="0.0"),
]
# HRA 96k and MA 192k branch points to learn the access-unit index rule
CSV = r"C:\Users\Callum\AppData\Local\Temp\claude\E--FormatProgram\cccb6f64-f44b-4c8d-85e6-cc21df66cdda\scratchpad\lab\csv"
for nm, base in (("hra96", dict(dest="bd", stream="hra", rate=96000, layout="5.1", kbps=4224)), ("ma192", dict(dest="bd", stream="ma", rate=192000, layout="5.1", kbps=0)), ("core", dict(dest="bd", stream="core", rate=48000, layout="5.1", kbps=1509)), ("ma96", dict(dest="bd", stream="ma", rate=96000, layout="5.1", kbps=0))):
    specs.append(dict(base, group="bitstream4", name="csv_" + nm, xml={"seamlessbranching-csvbranchpoints": True, "seamlessbranching-csvfilename": CSV + "\c.csv"}, extra_cmd=' -y"%s\c.csv"' % CSV))
lab.run_many(specs, timeout=120)
lab.Engine.close()
