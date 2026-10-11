import sweep1, lab
specs = []
for r in sweep1.rows():
    if r["stream"] != "express": continue
    for kb in r["rates"]:
        specs.append(dict(name="%s_%s_%d_%s_%d" % (r["dest"], r["stream"], r["rate"], r["layout"], kb), group="sweep_express",
                          dest=r["dest"], stream=r["stream"], rate=r["rate"], layout=r["layout"], kbps=kb))
print(len(specs), "cases", flush=True)
lab.run_many(specs, timeout=300)
lab.Engine.close()
