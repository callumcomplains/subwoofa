import sys
exec(open('xllhdr.py').read().split("d=open")[0])
def show(f, frame=40):
    d=open(f,'rb').read()
    i=-1
    for _ in range(frame+1): i=d.find(SYNC,i+1)
    b=B(d,(i+4)*8); b.g(4);hl=b.g(8)+1;fb=b.g(5)+1;b.g(fb);ns=b.g(4)+1
    b.g(4+4+5+2+1); mb=b.g(5)+1
    pos=i+hl
    for s in range(ns):
        b=B(d,pos*8)
        sz=b.g(10)+1; nch=b.g(4)+1; res=b.g(nch); b.g(5);b.g(5);b.g(4);b.g(2);b.g(2);prim=b.g(1)
        dm=b.g(1); emb=b.g(1) if dm else 0
        t=b.g(3) if (dm and prim) else None
        h=b.g(1)
        print(" set",s,"nch",nch,"prim",prim,"dm",dm,"emb",emb,"type",t,"hier",h, end=" ")
        if dm and prim:
            # unknown matrix length: print groups
            rest=sz*8-(b.p-pos*8)
            print()
            g=[]
            while (b.p-pos*8)+9<=sz*8-16 and len(g)<40:
                g.append(hex(b.g(9)))
            print("   groups",g)
        else:
            print()
        pos+=sz
if __name__=="__main__":
    show(sys.argv[1])
