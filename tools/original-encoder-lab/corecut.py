import sys
d=open(sys.argv[1],'rb').read()
i=d.find(b'STRMDATA')+16
S=bytes.fromhex('7ffe8001')
out=bytearray(); p=d.find(S,i); n=0
while p>=0 and p< len(d):
    h=int.from_bytes(d[p+4:p+9],'big')  # 40 bits
    # ftype1 short5 cpf1 nblks7 fsize14 ...
    fsize=((h>>(40-1-5-1-7-14))&0x3fff)+1
    if d[p:p+4]!=S: break
    out+=d[p:p+fsize]; n+=1
    # next AU: core then EXSS; find next sync after exss: search
    q=d.find(S,p+fsize)
    p=q
    if n>2000: break
open(sys.argv[2],'wb').write(out); print(n,len(out))
