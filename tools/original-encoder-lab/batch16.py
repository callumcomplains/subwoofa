import lab
G = "dmix2"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="hra", rate=48000, layout="7.1sr", kbps=5376); d.update(kw); d["name"] = name; return d
P = dict(left51primaryvalue="1.5", right51primaryvalue="4.5", center51primaryvalue="6.0", lfe51primaryvalue="9.0", ls51primaryvalue="12.0", rs51primaryvalue="0.0")
X = dict(ls51xchavalue="5.0", rs51xchbvalue="7.0", left51xchavalue="9.0", right51xchbvalue="10.0", center51xchavalue="11.0", center51xchbvalue="12.0", lfe51xchavalue="13.0", lfe51xchbvalue="14.0", rs51xchavalue="15.0", ls51xchbvalue="16.0", right51xchavalue="2.0", left51xchbvalue="4.0")
specs = []
for lay in ["7.1sr","7.1w","7.1h","7.0"]:
    specs.append(case("p_"+lay, layout=lay, xml=P))
    specs.append(case("x_"+lay, layout=lay, xml=X))
specs += [
 case("pl", xml=dict(left51primaryvalue="9.0")),
 case("pr", xml=dict(right51primaryvalue="9.0")),
 case("pc", xml=dict(center51primaryvalue="9.0")),
 case("plfe", xml=dict(lfe51primaryvalue="9.0")),
 case("pls", xml=dict(ls51primaryvalue="9.0")),
 case("prs", xml=dict(rs51primaryvalue="9.0")),
 case("pinf", xml=dict(left51primaryvalue="INF")),
]
lab.run_many(specs, timeout=180)
lab.Engine.close()
