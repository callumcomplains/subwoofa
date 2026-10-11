import compare, subprocess, sys
import xxchparse as X
S6="L=6,R=6,C=6,LFE=6,Ls=6,Rs=6"
cases=[("out/dmix/dm_default.dtshd","7.1sr",[]),
 ("out/dmix/dm_prim6.dtshd","7.1sr",["--dmx51-scale",S6]),
 ("out/dmix/dm_prim0.dtshd","7.1sr",["--dmx51-scale","L=0,R=0,C=0,LFE=0,Ls=0,Rs=0"]),
 ("out/dmix/dm_prim4p5.dtshd","7.1sr",["--dmx51-scale","L=4.5,R=4.5,C=4.5,LFE=4.5,Ls=4.5,Rs=4.5"]),
 ("out/dmix/dm_xcha6.dtshd","7.1sr",["--dmx51-a","Ls=6","--dmx51-b","Rs=6"]),
 ("out/dmix/dm_xcha0.dtshd","7.1sr",["--dmx51-a","Ls=0","--dmx51-b","Rs=0"]),
 ("out/dmix/dm_xcha1p5.dtshd","7.1sr",["--dmx51-a","Ls=1.5","--dmx51-b","Rs=1.5"]),
 ("out/dmix/dm_xcha_l.dtshd","7.1sr",["--dmx51-a","L=3,Ls=inf","--dmx51-b","R=3,Rs=inf"]),
 ("out/dmix/dm_xcha_c.dtshd","7.1sr",["--dmx51-a","C=6","--dmx51-b","C=6"]),
 ("out/dmix/dm_xcha_cross.dtshd","7.1sr",["--dmx51-a","Rs=6","--dmx51-b","Ls=6"]),
]
XA=["--dmx51-a","Ls=5,L=9,C=11,LFE=13,Rs=15,R=2","--dmx51-b","Rs=7,R=10,C=12,LFE=14,Ls=16,L=4"]
for lay in ["7.1sr","7.1w","7.1h","7.0"]:
    cases.append((f"out/dmix2/x_{lay}.dtshd",lay,XA))
for f,lay,extra in cases:
    out="native/z_%s.dtshd"%f.split("/")[-1][:-6]
    ok,msg=compare.native_encode(dict(layout=lay,stream="hra",dest="bd",rate=48000,kbps=5376),out,extra=extra)
    if not ok: print(f,"FAIL",msg[-200:]); continue
    a,b=X.parse(f),X.parse(out)
    keys=("core","nch","spkn","in_core","scale","dmask","codes","mask_bits")
    diff=[(k,a.get(k),b.get(k)) for k in keys if a.get(k)!=b.get(k)]
    print(f, "same" if not diff else diff)
