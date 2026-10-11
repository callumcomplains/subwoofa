"""rerun.py <log> <group>: re-run the cases that failed in a sweep log."""
import sys, re
import lab
log, group = sys.argv[1], sys.argv[2]
specs = []
for line in open(log):
    m = re.match(r"(\S+)\s+FAIL", line)
    if not m:
        continue
    name = m.group(1)
    dest, stream, rate, layout, kb = name.split("_")
    specs.append(dict(name=name, group=group, dest=dest, stream=stream, rate=int(rate), layout=layout, kbps=int(kb)))
print(len(specs), "cases")
lab.run_many(specs, timeout=300)
lab.Engine.close()
