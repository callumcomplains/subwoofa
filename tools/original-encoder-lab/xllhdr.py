"""Dump the XLL common header and channel-set headers (up to dmix) of a frame. usage: xllhdr.py file [frame#]"""
import sys
SYNC=bytes.fromhex("41a29547")
class B:
    def __init__(s,d,p=0): s.d,s.p=d,p
    def g(s,n):
        v=0
        for _ in range(n):
            v=(v<<1)|((s.d[s.p>>3]>>(7-(s.p&7)))&1); s.p+=1
        return v
d=open(sys.argv[1],'rb').read()
want=int(sys.argv[2]) if len(sys.argv)>2 else 40
i=-1
for _ in range(want+1):
    i=d.find(SYNC,i+1)
b=B(d,(i+4)*8)
ver=b.g(4); hl=b.g(8)+1; fb=b.g(5)+1; fl=b.g(fb)+1; ns=b.g(4)+1
print("frame at",i,"ver",ver,"hdr",hl,"frame_len",fl,"nsets",ns)
nseg=b.g(4); segl=b.g(4); nb4=b.g(5)+1; crcb=b.g(2); msb=b.g(1); mb=b.g(5)+1
print(" nseg_log",nseg,"seglen_log",segl,"nb4ssize",nb4,"crcband",crcb,"msb",msb,"mask_bits",mb, "lsbs", (b.g(4)+1) if msb else "-")
pos=i+hl
lower=0
for s in range(ns):
    st=pos
    b=B(d,pos*8)
    sz=b.g(10)+1
    nch=b.g(4)+1
    res=b.g(nch); br=b.g(5)+1; sb=b.g(5)+1; fs=b.g(4); interp=b.g(2); repl=b.g(2)
    if repl: print("  replacement!", b.g(1))
    prim=b.g(1); dm=b.g(1)
    print(f" set{s}: size {sz} nch {nch} resid {res:b} bitres {br}/{sb} fs {fs} interp {interp} repl {repl} primary {prim} dmix_present {dm}")
    if dm:
        emb=b.g(1); hier=b.g(1)
        print("   dmix applied",emb,"hier",hier)
        rows=[]
        for r in range(lower):
            sc=b.g(9); cs=[b.g(9) for _ in range(nch)]
            rows.append((sc,cs))
            print("   row",r,"scale",hex(sc),"coef",[hex(c) for c in cs])
    else:
        hier=b.g(1); print("   hier",hier)
    me=b.g(1); mask=b.g(mb)
    print("   mask enabled",me,"mask",hex(mask))
    lower+=nch
    pos+=sz
