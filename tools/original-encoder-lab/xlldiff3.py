import compare, subprocess, sys, re
def dump(p):
    t=subprocess.run([sys.executable,"xllhdr.py",p,"40"],capture_output=True,text=True).stdout
    return [re.sub(r"size \d+ ","",l) for l in t.splitlines() if not l.startswith("frame at") and "nseg_log" not in l]
def dumpu(p):
    return subprocess.run([sys.executable,"xllu.py",p],capture_output=True,text=True).stdout.splitlines()
E=["--embed-stereo"]
cases=[
 ("out/emb2/c_l0.dtshd","5.1",E+["--dmx20-a","L=0","--dmx20-b","R=0"],dump),
 ("out/emb2/c_l6.dtshd","5.1",E+["--dmx20-a","L=6","--dmx20-b","R=1.5"],dump),
 ("out/emb2/c_c3.dtshd","5.1",E+["--dmx20-a","C=3","--dmx20-b","C=9"],dump),
 ("out/emb2/c_cinf.dtshd","5.1",E+["--dmx20-a","C=inf","--dmx20-b","C=inf"],dump),
 ("out/emb2/c_lfe.dtshd","5.1",E+["--dmx20-a","LFE=6","--dmx20-b","LFE=12"],dump),
 ("out/emb2/c_ls.dtshd","5.1",E+["--dmx20-a","Ls=3,Rs=4.5","--dmx20-b","Ls=9,Rs=7.5"],dump),
 ("out/emb2/c_rl.dtshd","5.1",E+["--dmx20-a","R=6","--dmx20-b","L=12"],dump),
 ("out/emb2/c_ltrt.dtshd","5.1",E+["--stereo-lt-rt"],dump),
 ("out/emb2/cu_ltrt.dtshd","5.1",["--stereo-matrix","--stereo-lt-rt"],dumpu),
 ("out/emb2/cu_loro.dtshd","5.1",["--stereo-matrix"],dumpu),
 ("out/emb3/p_7.1w.dtshd","7.1w",["--dmx51-scale","L=1.5,R=4.5,C=6,LFE=9,Ls=12,Rs=0"],dump),
 ("out/emb3/p_7.1ch.dtshd","7.1ch",["--dmx51-scale","L=1.5,R=4.5,C=6,LFE=9,Ls=12,Rs=0"],dump),
 ("out/emb3/x_7.1w.dtshd","7.1w",["--dmx51-a","Ls=5,L=9,C=11,LFE=13,Rs=15","--dmx51-b","Rs=7,R=10,C=12,LFE=14,Ls=16"],dump),
 ("out/emb3/x_7.1sr.dtshd","7.1sr",["--dmx51-a","Ls=5,L=9,C=11,LFE=13,Rs=15","--dmx51-b","Rs=7,R=10,C=12,LFE=14,Ls=16"],dump),
 ("out/emb3/x_7.1ch.dtshd","7.1ch",["--dmx51-a","Ls=5,L=9,C=11,LFE=13,Rs=15","--dmx51-b","Rs=7,R=10,C=12,LFE=14,Ls=16"],dump),
 ("out/emb3/e_7.1.dtshd","7.1",E,dump),
 ("out/emb3/e_7.1ch.dtshd","7.1ch",E,dump),
 ("out/emb3/e_7.1hs.dtshd","7.1hs",E,dump),
]
for f,lay,extra,d in cases:
    out="native/y_%s.dtshd"%f.split("/")[-1][:-6]
    ok,msg=compare.native_encode(dict(layout=lay,stream="ma",dest="bd",rate=48000,kbps=1509),out,extra=extra)
    if not ok: print(f,"FAIL",msg[-200:]); continue
    a,b=d(f),d(out)
    if d is dumpu:
        a=[re.sub(r"groups.*","",x) if False else x for x in a]
        trim=lambda L:[ (x if "groups" not in x else str(re.findall(r"'(0x[0-9a-f]+)'",x)[:12])) for x in L]
        a,b=trim(a),trim(b)
    print(f,"same" if a==b else "DIFF")
    if a!=b:
        for x in a:
            if x not in b: print("  orig:",x)
        for x in b:
            if x not in a: print("  ours:",x)
