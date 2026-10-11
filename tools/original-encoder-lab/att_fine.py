import json, lab, exssparse as E, os
SEC = dict(group="attfine", dest="bd2", stream="express", rate=48000, layout="sa1.0", kbps=96, tc_end="00:00:02:00")
out = {}
vals = [round(0.1 * i, 1) for i in range(0, 631)]
import re as _re
tmpl = open("jobs/bitstream/x1_default/job.cfg").read()
for db in vals:
    cfg = _re.sub(r"(DTS_CHCFG_\w+=\s+)6\.0(\s)", lambda m: m.group(1) + "%.1f" % db + m.group(2), tmpl)
    s = dict(SEC); s["cfg_template"] = cfg; s.update(name="a_%s" % str(db).replace(".", "p"), pa_att=True, pa_att_db="%.1f" % db, fade_down="0.0", fade_up="0.0")
    r = lab.run_case(s, timeout=60)
    lab.Engine.drain()
    if not r["ok"] or not r["out"]:
        print("FAIL", db, r.get("log", "")[:80]); continue
    data = open(r["out"][0], "rb").read()
    fr = E.find_frames(data)
    out[db] = E.parse_frame(data, fr[0])[0]["assets"][0]["mix"]["scales"][0]
    os.remove(r["out"][0])
    if len(out) % 50 == 0:
        print(len(out), flush=True)
        json.dump(out, open("att_fine.json", "w"))
json.dump(out, open("att_fine.json", "w"))
lab.Engine.close()
print("done")
