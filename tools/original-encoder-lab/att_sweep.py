import json, lab, exssparse as E
SEC = dict(group="att", dest="bd2", stream="express", rate=48000, layout="sa1.0", kbps=96)
specs = []
for db in ["0.0", "1.0", "2.0", "3.0", "4.0", "5.0", "6.0", "8.0", "10.0", "12.0", "15.0", "18.0", "20.0", "24.0", "30.0", "36.0", "45.0", "60.0", "80.0", "INF"]:
    s = dict(SEC); s.update(name="att_%s" % db.replace(".", "p"), pa_att=True, pa_att_db=db, fade_down="0.0", fade_up="0.0")
    specs.append(s)
for fd, fu in [("1.0", "0.0"), ("2.0", "0.0"), ("4.0", "0.0"), ("0.0", "2.0"), ("0.0", "4.0"), ("0.5", "0.5"), ("8.0", "0.0")]:
    s = dict(SEC); s.update(name="fade_30_%s_%s" % (fd.replace(".", "p"), fu.replace(".", "p")), pa_att=True, pa_att_db="30.0", fade_down=fd, fade_up=fu)
    specs.append(s)
for db, fd, fu in [("12.0", "2.0", "2.0"), ("6.0", "2.0", "2.0")]:
    s = dict(SEC); s.update(name="fade_%s_%s_%s" % (db.replace(".", "p"), fd.replace(".", "p"), fu.replace(".", "p")), pa_att=True, pa_att_db=db, fade_down=fd, fade_up=fu)
    specs.append(s)
res = lab.run_many(specs, timeout=120)
lab.Engine.close()
out = {}
for r in res:
    if not r["ok"] or not r["out"]:
        continue
    data = open(r["out"][0], "rb").read()
    fr = E.find_frames(data)
    try:
        codes = [E.parse_frame(data, o)[0]["assets"][0]["mix"]["scales"][0] for o in fr]
    except Exception as e:
        codes = str(e)
    out[r["spec"]["name"]] = codes
json.dump(out, open("att_codes.json", "w"))
for k, v in out.items():
    print(k, v if isinstance(v, str) else (v[:3], "...", v[-3:], "min", min(v), "max", max(v)))
