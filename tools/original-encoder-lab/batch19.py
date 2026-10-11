import lab
G = "aux"
def case(name, **kw):
    d = dict(group=G, dest="bd", stream="core", rate=48000, layout="5.1", kbps=1509, use20=True); d.update(kw); d["name"] = name; return d
specs = []
for lay in ["1.0","1.1","2.0","2.0lt","2.1","3.0","3.1","3.0s","3.1s","4.0","4.1","4.0cs","4.1cs","5.0","5.1","6.0m","6.1m"]:
    specs.append(case("c_"+lay, layout=lay))
specs.append(case("es_6.1d", stream="es", layout="6.1d"))
specs.append(case("es_6.0d", stream="es", layout="6.0d"))
specs.append(case("es_6.1m", stream="es", layout="6.1m"))
for lay in ["7.1w","7.0","7.1sr","7.1oh"]:
    specs.append(case("hra_"+lay, stream="hra", layout=lay, kbps=5376))
specs.append(case("dvd51", dest="dvd"))
specs.append(case("dvd20", dest="dvd", layout="2.0"))
specs.append(case("core96_51", stream="core96", rate=96000))
specs.append(case("ltrt", embed20_ltrt=True))
specs.append(case("loro", xml=dict(is2ChDmixLoRo=True)))
specs.append(case("x1", xml=dict(left20xchavalue="9.0", right20xchbvalue="1.5", center20xchavalue="3.0", center20xchbvalue="12.0", lfe20xchavalue="5.0", lfe20xchbvalue="7.0", ls20xchavalue="10.0", ls20xchbvalue="4.0", rs20xchavalue="2.0", rs20xchbvalue="8.0", right20xchavalue="11.0", left20xchbvalue="13.0")))
specs.append(case("x2", layout="3.0", xml=dict(left20xchavalue="9.0", right20xchbvalue="1.5", center20xchavalue="3.0", center20xchbvalue="12.0")))
lab.run_many(specs, timeout=180)
lab.Engine.close()
