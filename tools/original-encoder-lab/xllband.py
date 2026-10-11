import sys
exec(open('xllhdr.py').read().split("d=open")[0])
def show(f, frame):
    d=open(f,'rb').read()
    i=-1
    for _ in range(frame+1): i=d.find(SYNC,i+1)
    b=B(d,(i+4)*8); b.g(4);hl=b.g(8)+1;fb=b.g(5)+1;b.g(fb);ns=b.g(4)+1
    b.g(4+4+5+2); msb=b.g(1); mb=b.g(5)+1
    if msb: b.g(4)
    pos=i+hl
    for s in range(ns):
        b=B(d,pos*8)
        sz=b.g(10)+1; nch=b.g(4)+1; res=b.g(nch); br=b.g(5)+1; sb=b.g(5)+1; fs=b.g(4); b.g(2); b.g(2); prim=b.g(1)
        dm=b.g(1)
        lower=sum(0 for _ in [])
        if dm:
            emb=b.g(1); 
            if prim: t=b.g(3)
            hier=b.g(1)
        else:
            hier=b.g(1)
        print(f"set{s} nch{nch} prim{prim} dm{dm}", end=" ")
        # skip matrix/rows: unknown count -> pass rows param
        rows=int(sys.argv[3+s]) if len(sys.argv)>3+s else 0
        if dm and not prim:
            for _ in range(rows*(1+nch)): b.g(9)
        elif dm and prim:
            for _ in range(2*nch): b.g(9)
        me=b.g(1); mask=b.g(mb)
        left=sz*8-(b.p-pos*8)-16
        print("mask",hex(mask),"remaining bits(excl crc)",left, ''.join(str(b.g(1)) for _ in range(left)))
        pos+=sz
show(sys.argv[1], int(sys.argv[2]))
