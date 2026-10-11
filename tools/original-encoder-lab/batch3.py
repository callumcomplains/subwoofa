import lab
CSV = r"C:\Users\Callum\AppData\Local\Temp\claude\E--FormatProgram\cccb6f64-f44b-4c8d-85e6-cc21df66cdda\scratchpad\lab\csv"
MA51 = dict(group="bitstream3", dest="bd", stream="ma", rate=48000, layout="5.1", kbps=0)
specs = []
for n in "abcdefg":
    specs.append(dict(MA51, name="csv_" + n, xml={"seamlessbranching-csvbranchpoints": True, "seamlessbranching-csvfilename": CSV + "\%s.csv" % n}, extra_cmd=' -y"%s\%s.csv"' % (CSV, n)))
specs.append(dict(MA51, name="csv_two", xml={"seamlessbranching-csvbranchpoints": True, "seamlessbranching-csvfilename": CSV + "\two.csv"}, extra_cmd=' -y"%s\two.csv"' % CSV))
lab.run_many(specs, timeout=120)
lab.Engine.close()
