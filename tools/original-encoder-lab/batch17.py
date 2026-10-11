import lab
G = "emb4"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="ma", rate=48000, layout="7.1w", kbps=1509, embed20=True, use20=True); d.update(kw); d["name"] = name; return d
specs = [
 case("pl", xml=dict(left51primaryvalue="9.0")),
 case("pc", xml=dict(center51primaryvalue="9.0")),
 case("pls", xml=dict(ls51primaryvalue="9.0")),
 case("plfe", xml=dict(lfe51primaryvalue="9.0")),
 case("all", xml=dict(left51primaryvalue="1.5", right51primaryvalue="4.5", center51primaryvalue="6.0", lfe51primaryvalue="9.0", ls51primaryvalue="12.0", rs51primaryvalue="0.0")),
 case("st", xml=dict(left20xchavalue="9.0", right20xchbvalue="1.5")),
 case("xa", xml=dict(ls51xchavalue="9.0", rs51xchbvalue="1.5", center51xchavalue="4.5")),
]
lab.run_many(specs, timeout=180)
lab.Engine.close()
