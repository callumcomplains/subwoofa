import compare, subprocess, sys, glob, os, re
def dump(p):
    t=subprocess.run([sys.executable,"xllhdr.py",p,"40"],capture_output=True,text=True).stdout
    return [re.sub(r"size \d+ ","",l) for l in t.splitlines() if not l.startswith("frame at") and "nseg_log" not in l]
def dumpu(p):
    t=subprocess.run([sys.executable,"xllu.py",p],capture_output=True,text=True).stdout
    return t.splitlines()
bad=0
def run(tag, files, extra, dumper):
    global bad
    for f in files:
        lay=os.path.basename(f).split("_",1)[1].rsplit(".dtshd",1)[0]
        lay=lay.split("_")[0] if "sweep1_max" in f else lay
        if "sweep1_max" in f: lay=os.path.basename(f).split("_")[3]
        spec=dict(layout=lay,stream="ma",dest="bd",rate=48000,kbps=1509)
        out="native/x_%s_%s.dtshd"%(tag,lay)
        ok,msg=compare.native_encode(spec,out,extra=extra)
        if not ok: print(tag,lay,"native fail",msg[-100:]); continue
        a,b=dumper(f),dumper(out)
        if a!=b:
            bad+=1; print("==",tag,lay)
            for x in a:
                if x not in b: print("  orig:",x)
            for x in b:
                if x not in a: print("  ours:",x)
mode=sys.argv[1]
if mode=="plain": run("plain",sorted(glob.glob("out/sweep1_max/bd_ma_48000_*_1509.dtshd")),[],dump)
if mode=="emb": run("emb",sorted(glob.glob("out/emb2/e_*.dtshd"))+sorted(glob.glob("out/emb3/e_*.dtshd")),["--embed-stereo"],dump)
if mode=="mat": run("mat",sorted(glob.glob("out/emb2/u_*.dtshd")),["--stereo-matrix"],dumpu)
print("bad",bad)
