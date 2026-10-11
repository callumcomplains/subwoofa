import sys, lab, sweep1
want = sys.argv[1].split(",")
group = sys.argv[2]
specs = []
for r in sweep1.rows():
    if r["layout"] in want:
        kb = max(r["rates"])
        specs.append(dict(name="%s_%s_%d_%s_%d" % (r["dest"], r["stream"], r["rate"], r["layout"], kb), group=group, dest=r["dest"], stream=r["stream"], rate=r["rate"], layout=r["layout"], kbps=kb))
print(len(specs), flush=True)
lab.run_many(specs, timeout=300)
lab.Engine.close()
