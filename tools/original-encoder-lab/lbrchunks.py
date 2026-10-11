import sys
import exssparse as E
def frames(path, k):
    d=open(path,'rb').read(); fr=E.find_frames(d)
    o=fr[k]; f,_=E.parse_frame(d,o)
    start=o+f['header_size']; size=f['asset_sizes'][0]
    return d[start:start+size]
def walk(a):
    out=[]
    assert a[:4]==bytes.fromhex('0a801921'), a[:4].hex()
    p=16
    assert a[p]==0x84
    ln=int.from_bytes(a[p+1:p+3],'big')
    body=a[p+5:p+3+ln]
    q=0
    while q+2<=len(body):
        i=body[q]
        if i&0x80:
            l=int.from_bytes(body[q+1:q+3],'big'); hl=3
        else:
            l=body[q+1]; hl=2
        if i==0 and l==0: break
        out.append((hex(i&0x7f), 'L' if i&0x80 else 'S', l)); q+=hl+l
        if q>len(body): out.append('overrun'); break
    return a[:16].hex(), out
if __name__=="__main__":
    k=int(sys.argv[-1])
    for path in sys.argv[1:-1]:
        h,c=walk(frames(path,k))
        print(path.split('/')[-1], h); print("  ",c)
