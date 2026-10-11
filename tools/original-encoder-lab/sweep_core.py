import sweep1, lab
specs = []
for r in sweep1.rows():
    if r["dest"] != "bd" or r["stream"] != "core": continue
    if r["layout"] not in ("2.0", "5.1", "3.0", "4.1", "5.0"): continue
    for kb in r["rates"]:
        specs.append(dict(name="%s_%s_%d_%s_%d" % (r["dest"], r["stream"], r["rate"], r["layout"], kb), group="sweep_core",
                          dest=r["dest"], stream=r["stream"], rate=r["rate"], layout=r["layout"], kbps=kb))
print(len(specs), "cases", flush=True)
lab.run_many(specs, timeout=300, verbose=False)
lab.Engine.close()
print("done")
