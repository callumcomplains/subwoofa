import lab
SEC = dict(group="bitstream5", dest="bd2", stream="express", rate=48000, kbps=128)
specs = [
 dict(SEC, name="st_20_fades", layout="sa2.0", pa_att=True, pa_att_db="30.0", fade_down="2.0", fade_up="4.0"),
 dict(SEC, name="st_20_att12", layout="sa2.0", pa_att=True, pa_att_db="12.0", fade_down="0.0", fade_up="0.0"),
 dict(SEC, name="st_51_fades", layout="sa5.1", kbps=256, pa_att=True, pa_att_db="30.0", fade_down="2.0", fade_up="4.0"),
 dict(SEC, name="st_20_map", layout="sa2.0", pa_att=True, pa_att_db="6.0", fade_down="0.0", fade_up="0.0",
      xml=dict(monoMetadataCValue="INF", monoMetadataLValue="0.0", monoMetadataRValue="6.0", monoMetadataLsValue="12.0", monoMetadataRsValue="INF", monoMetadataLFEValue="INF")),
]
lab.run_many(specs, timeout=120)
lab.Engine.close()
