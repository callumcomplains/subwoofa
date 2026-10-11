import lab
G = "emb2"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="ma", rate=48000, layout="5.1", kbps=1509); d.update(kw); d["name"] = name; return d
def x(**kw): return dict(xml=kw)
specs = []
for lay in ["5.1","5.0","4.1","4.0","3.1","3.0","2.1","6.1m","6.1d","6.0m","7.1sr","7.1w","7.0","7.1oh","7.1h"]:
    specs.append(case("e_"+lay, layout=lay, embed20=True, use20=True))
    specs.append(case("u_"+lay, layout=lay, use20=True))
specs += [
    case("c_ltrt", embed20=True, use20=True, embed20_ltrt=True),
    case("c_loro", embed20=True, use20=True, xml=dict(is2ChDmixLoRo=True)),
    case("cu_ltrt", use20=True, embed20_ltrt=True),
    case("cu_loro", use20=True, xml=dict(is2ChDmixLoRo=True)),
    case("c_l0", embed20=True, use20=True, xml=dict(left20xchavalue="0.0", right20xchbvalue="0.0")),
    case("c_l6", embed20=True, use20=True, xml=dict(left20xchavalue="6.0", right20xchbvalue="1.5")),
    case("c_c3", embed20=True, use20=True, xml=dict(center20xchavalue="3.0", center20xchbvalue="9.0")),
    case("c_cinf", embed20=True, use20=True, xml=dict(center20xchavalue="INF", center20xchbvalue="INF")),
    case("c_lfe", embed20=True, use20=True, xml=dict(lfe20xchavalue="6.0", lfe20xchbvalue="12.0")),
    case("c_ls", embed20=True, use20=True, xml=dict(ls20xchavalue="3.0", ls20xchbvalue="9.0", rs20xchavalue="4.5", rs20xchbvalue="7.5")),
    case("c_rl", embed20=True, use20=True, xml=dict(right20xchavalue="6.0", left20xchbvalue="12.0")),
]
lab.run_many(specs, timeout=180)
lab.Engine.close()
