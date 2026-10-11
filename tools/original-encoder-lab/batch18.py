import lab
G = "emb5"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="core", rate=48000, layout="5.1", kbps=1509); d.update(kw); d["name"] = name; return d
specs = []
for st,lay,kb,rate in [("core","5.1",1509,48000),("es","6.1m",1509,48000),("core96","5.1",1509,96000),("hra","5.1",3840,48000),("hra","5.1",4224,96000),("express","5.1",192,48000)]:
    base=dict(stream=st,layout=lay,kbps=kb,rate=rate)
    specs.append(case(f"{st}{rate}_plain", **base))
    specs.append(case(f"{st}{rate}_u20", use20=True, **base))
    specs.append(case(f"{st}{rate}_e20", embed20=True, **base))
    specs.append(case(f"{st}{rate}_eu20", embed20=True, use20=True, **base))
specs.append(case("dvd_plain", dest="dvd", kbps=1509))
specs.append(case("dvd_eu20", dest="dvd", kbps=1509, embed20=True, use20=True))
lab.run_many(specs, timeout=180)
lab.Engine.close()
