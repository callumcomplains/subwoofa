import json, lab, exssparse as E
SEC = dict(group="fade", dest="bd2", stream="express", rate=48000, layout="sa1.0", kbps=96)
specs = []
for A in ["3.0", "6.0", "9.0", "12.0", "15.0", "18.0", "24.0", "30.0", "45.0", "60.0"]:
    for T in ["8.0", "4.0", "2.0", "9.0"]:
        s = dict(SEC); s.update(name="f_%s_%s" % (A.replace(".", "p"), T.replace(".", "p")), pa_att=True, pa_att_db=A, fade_down=T, fade_up="0.0")
        specs.append(s)
res = lab.run_many(specs, timeout=120, verbose=False)
lab.Engine.close()
out = {}
for r in res:
    if not r["ok"] or not r["out"]:
        print("FAIL", r["spec"]["name"]); continue
    data = open(r["out"][0], "rb").read()
    fr = E.find_frames(data)
    out[r["spec"]["name"]] = [E.parse_frame(data, o)[0]["assets"][0]["mix"]["scales"][0] for o in fr]
json.dump(out, open("fade_codes.json", "w"))
print(len(out), "ok")
