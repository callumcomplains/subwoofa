import sweep1, lab, os
specs = []
for r in sweep1.rows():
    if r["dest"] != "bd" or r["stream"] != "hra": continue
    if r["layout"] not in ("7.1",): continue
    for kb in r["rates"]:
        specs.append(dict(name="%s_%s_%d_%s_%d" % (r["dest"], r["stream"], r["rate"], r["layout"], kb), group="hra_rear2",
                          dest=r["dest"], stream=r["stream"], rate=r["rate"], layout=r["layout"], kbps=kb))
print(len(specs), "cases", flush=True)
lab.run_many(specs, timeout=300)
lab.Engine.close()
