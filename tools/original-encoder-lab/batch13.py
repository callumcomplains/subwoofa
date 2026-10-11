import lab
G = "emb"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="ma", rate=48000, layout="5.1", kbps=1509); d.update(kw); d["name"] = name; return d
def x(**kw): return dict(xml=kw)
specs = [
    case("ma51_plain"),
    case("ma51_emb", embed20=True),
    case("ma51_emb_ltrt", embed20=True, embed20_ltrt=True),
    case("ma51_emb_loro", embed20=True, xml=dict(is2ChDmixLoRo=True)),
    case("ma51_use20", use20=True),
    case("ma51_emb_use20", embed20=True, use20=True),
    case("ma71_plain", layout="7.1sr"),
    case("ma71_emb", layout="7.1sr", embed20=True),
    case("ma71_emb_ltrt", layout="7.1sr", embed20=True, embed20_ltrt=True),
    case("hra51_plain", stream="hra", kbps=3840),
    case("hra51_emb", stream="hra", kbps=3840, embed20=True),
    case("hra51_emb_ltrt", stream="hra", kbps=3840, embed20=True, embed20_ltrt=True),
]
lab.run_many(specs, timeout=180)
lab.Engine.close()
