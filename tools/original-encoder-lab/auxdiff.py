import compare, coreaux, os
M=["--stereo-matrix"]
X1=["--dmx20-a","L=9,C=3,LFE=5,Ls=10,Rs=2,R=11","--dmx20-b","R=1.5,C=12,LFE=7,Ls=4,Rs=8,L=13"]
cases=[("out/aux/c_5.0.dtshd","core","5.0",1509,48000,"bd",M),
 ("out/aux/c_5.1.dtshd","core","5.1",1509,48000,"bd",M),
 ("out/aux/c_4.0.dtshd","core","4.0",1509,48000,"bd",M),
 ("out/aux/c_3.0.dtshd","core","3.0",1509,48000,"bd",M),
 ("out/aux/es_6.1d.dtshd","es","6.1d",1509,48000,"bd",M),
 ("out/aux/dvd51.cpt","core","5.1",1509,48000,"dvd",M),
 ("out/aux/core96_51.dtshd","core96","5.1",1509,96000,"bd",M),
 ("out/aux/ltrt.dtshd","core","5.1",1509,48000,"bd",M+["--stereo-lt-rt"]),
 ("out/aux/loro.dtshd","core","5.1",1509,48000,"bd",M),
 ("out/aux/x1.dtshd","core","5.1",1509,48000,"bd",M+X1),
 ("out/emb5/hra48000_u20.dtshd","hra","5.1",3840,48000,"bd",M),
 ("out/emb5/hra96000_u20.dtshd","hra","5.1",4224,96000,"bd",M),
 ("out/emb2/u_5.1.dtshd","ma","5.1",1509,48000,"bd",M),
 ("out/emb2/e_5.1.dtshd","ma","5.1",1509,48000,"bd",M+["--embed-stereo"]),
 ("out/aux/hra_7.1w.dtshd","hra","7.1w",5376,48000,"bd",M),
 ("out/emb2/u_7.1w.dtshd","ma","7.1w",1509,48000,"bd",M),
]
for f,st,lay,kb,rate,dest,extra in cases:
    ext=f.rsplit(".",1)[1]
    out="native/au_%s_%s_%s.%s"%(st,lay,dest,ext)
    ok,msg=compare.native_encode(dict(layout=lay,stream=st,dest=dest,rate=rate,kbps=kb),out,extra=extra)
    if not ok: print(f,"FAIL",msg[-150:]); continue
    a=coreaux.parse(f); b=coreaux.parse(out)
    keys=("auxf","aux_at","aux_len","type","codes","crc","pre")
    d=[(k,a.get(k),b.get(k)) for k in keys if a.get(k)!=b.get(k)]
    print(os.path.basename(f), "same" if not d else d)
