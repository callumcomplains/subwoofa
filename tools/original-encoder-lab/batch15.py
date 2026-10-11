import lab
G = "emb3"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="ma", rate=48000, layout="7.1w", kbps=1509); d.update(kw); d["name"] = name; return d
specs = []
for lay in ["7.1","7.1ch","7.1hs","7.1sr"]:
    specs.append(case("e_"+lay, layout=lay, embed20=True, use20=True))
specs.append(case("e_7.1sr_nouse20", layout="7.1sr", embed20=True))
specs.append(case("e_7.1w_nouse20", layout="7.1w", embed20=True))
# custom 5.1 downmix values on 7.1 variants
for lay in ["7.1w","7.1ch","7.1sr"]:
    specs.append(case("p_"+lay, layout=lay, xml=dict(left51primaryvalue="1.5", right51primaryvalue="4.5", center51primaryvalue="6.0", lfe51primaryvalue="9.0", ls51primaryvalue="12.0", rs51primaryvalue="0.0")))
    specs.append(case("x_"+lay, layout=lay, xml=dict(ls51xchavalue="5.0", rs51xchbvalue="7.0", left51xchavalue="9.0", right51xchbvalue="10.0", center51xchavalue="11.0", center51xchbvalue="12.0", lfe51xchavalue="13.0", lfe51xchbvalue="14.0", rs51xchavalue="15.0", ls51xchbvalue="16.0")))
lab.run_many(specs, timeout=180)
lab.Engine.close()
