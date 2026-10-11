import sys
NCH={0:1,1:2,2:2,3:2,4:2,5:3,6:3,7:4,8:4,9:5,10:6,11:6,12:6,13:7,14:8,15:8}
class B:
    def __init__(s,d,p=0): s.d,s.p=d,p
    def g(s,n):
        v=0
        for _ in range(n):
            v=(v<<1)|((s.d[s.p>>3]>>(7-(s.p&7)))&1); s.p+=1
        return v
def parse(path, frame=40):
    d=open(path,'rb').read(); i=d.find(b'STRMDATA')+16; A=d[i:]
    S=bytes.fromhex('7ffe8001')
    p=A.find(S)
    for _ in range(frame): 
        fs=((int.from_bytes(A[p+4:p+9],'big')>>(40-1-5-1-7-14))&0x3fff)+1
        q=A.find(S,p+fs); p=q
    b=B(A,(p+4)*8); b.g(1+5+1+7)
    fs=b.g(14)+1; amode=b.g(6); sf=b.g(4); rate=b.g(5); b.g(1+1+1); auxf=b.g(1); b.g(1+3+1+1); lff=b.g(2)
    nch=NCH.get(amode,0)+(1 if lff else 0)
    out=dict(fs=fs,amode=amode,nch=nch,auxf=auxf)
    if auxf:
        fr=A[p:p+fs]
        k=fr.find(bytes.fromhex('9a1105a0'))
        out['aux_at']=k
        out['aux_len']=fs-k-4
        bb=B(fr,(k+4)*8)
        ts=bb.g(1); dm=bb.g(1); t=bb.g(3)
        out.update(ts=ts,dm=dm,type=t)
        if dm:
            m=2
            codes=[hex(bb.g(9)) for _ in range(m*nch)]
            out['codes']=[codes[:nch],codes[nch:]]
        # bytes before sync
        out['pre']=fr[k-6:k].hex()
        bb.p=(bb.p+7)//8*8
        out['crc']=hex(bb.g(16)); out['after']=fr[(bb.p//8):(bb.p//8)+8].hex()
        out['rest_bytes_after_crc']=fs-bb.p//8
    return out
if __name__=="__main__":
    for f in sys.argv[1:]:
        print(f.split("/")[-1], parse(f))
