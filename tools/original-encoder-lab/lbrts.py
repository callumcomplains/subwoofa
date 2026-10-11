"""Walk the time-sample chunk 0x60 (mono pair 0) of an Express frame: bands, levels, coding method, bits."""
import re, sys
import lbrchunks as L
T=open('E:/FormatProgram/subwoofa/crates/sw-codec-dts/src/lbr_tables.rs').read()
def table(name):
    m=re.search(r'pub const %s: \[\(u8, u8\); \d+\] = \[(.*?)\];'%name,T,re.S)
    return [(int(a),int(b)) for a,b in re.findall(r'\((\d+), (\d+)\)',m.group(1))]
def canon(tab):
    out={}; code=0; prev=tab[0][1]
    for i,(sym,ln) in enumerate(tab):
        if i>0:
            code+=1
            code = code<<(ln-prev) if ln>prev else code>>(prev-ln)
        prev=ln
        out[(code,ln)]=sym
    return out
G3=canon(table('HUFF_GRID_3')); RSD=canon(table('HUFF_RSD')); G2=canon(table('HUFF_GRID_2'))
class R:
    def __init__(s,b): s.b=b; s.p=0
    def bit(s):
        v=(s.b[s.p>>3]>>(s.p&7))&1; s.p+=1; return v
    def get(s,n):
        v=0
        for i in range(n): v|=s.bit()<<i
        return v
    def left(s): return len(s.b)*8-s.p
    def vlc(s,t):
        code=0
        for ln in range(1,17):
            code=(code<<1)|s.bit()
            if (code,ln) in t: return t[(code,ln)]
        raise ValueError("bad vlc")
def levels(profile,nsb,rate):
    ol=(profile>>3)&7; st=profile>>6
    q=[]
    for sb in range(nsb):
        f=sb*rate//nsb
        a=18000//(12*f//1000+100+40*st)+20*ol
        q.append(1 if a<=95 else 2 if a<=140 else 3 if a<=180 else 4 if a<=230 else 5)
    return q
def walk(path,k,bl,res=1):
    a=L.frames(path,k)
    c=L.walk(a)  # just validates
    # re-get chunks
    p=16; ln=int.from_bytes(a[p+1:p+3],'big'); body=a[p+5:p+3+ln]; q=0; ch={}
    while q+2<=len(body):
        i=body[q]
        if i&0x80: l=int.from_bytes(body[q+1:q+3],'big'); hl=3
        else: l=body[q+1]; hl=2
        if i==0 and l==0: break
        ch[i&0x7f]=body[q+hl:q+hl+l]; q+=hl+l
    nsb=32>>bl; rate=48000>>bl
    prof=ch[0x40][0]; lv=levels(prof,nsb,rate)
    # reorder lower 8 omitted (levels for positions>=8 are plain)
    max_mono=nsb*14000//(rate//2)
    g3start=nsb*{0:16000,1:18000,2:24000}[res]//(rate//2) if res is not None else nsb
    r=R(ch[0x60])
    S2=[0]*10+[1]*8+[2]*14
    ng2=S2[nsb-1]+1
    gsyms=0
    for g in range(1,min(3,ng2)):
        for i in range(8):
            if r.bit():
                for j in range(8): r.vlc(G2); gsyms+=1
    pres=set([0,1])  # grid 3 bands already parsed in the 0x50 chunk (bands 4,5)
    out=[]; pos=6
    idxbits=3+(2-bl)
    while r.left()>=28 and pos<nsb:
        idx=r.get(idxbits); idx=max(idx,6)
        if pos<max_mono and pos==12:
            for sg in range(0,g3start-4):
                if sg not in pres:
                    for i in range(8): r.vlc(G3)
                    pres.add(sg)
        elif pos<max_mono and pos<12 and idx>=4:
            if idx-4 not in pres:
                for i in range(8): r.vlc(G3)
                pres.add(idx-4)
        lvl=lv[pos]; start=r.p
        meth=r.bit()
        if lvl==1:
            nb=min(r.left()//8,16); r.get(8*nb); n=nb*8
        elif lvl==2:
            if meth:
                n=0
                while n<128 and r.left()>=2:
                    if r.bit(): r.bit()
                    n+=1
            else:
                nb=min(r.left()//8,26); r.get(8*nb); n=nb*5
        elif lvl==3:
            nb=min(r.left()//7,43); r.get(7*nb); n=nb*3
        elif lvl==4:
            n=0
            while n<128 and r.left()>=6:
                r.vlc(RSD); n+=1
        else:
            nb=min(r.left()//4,128); r.get(4*nb); n=nb
        out.append((pos,idx,lvl,meth,r.p-start,n))
        pos+=1
    return prof,out,gsyms,r.left()
if __name__=="__main__":
    f=sys.argv[1]; k=int(sys.argv[2]); bl=int(sys.argv[3])
    prof,out,gs,left=walk(f,k,bl); print(hex(prof),'grid2 syms',gs,'left bits',left); 
    for o in out: print(o)
