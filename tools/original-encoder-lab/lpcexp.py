import numpy as np, re
T=open('E:/FormatProgram/subwoofa/crates/sw-codec-dts/src/lbr_tables.rs').read()
def arr(name):
    m=re.search(r'pub const %s: \[f32; \d+\] = \[(.*?)\];'%name,T,re.S)
    return np.array([float(x) for x in m.group(1).split(',')])
QA=arr('QUANT_AMP'); L8=arr('RSD_LEVEL_8'); L16=arr('RSD_LEVEL_16'); L5=arr('RSD_LEVEL_5'); L3=arr('RSD_LEVEL_3')
ALPHA={3:L5,4:L8,5:L16}
d=np.fromfile('ts_dump.bin',dtype='<f8').reshape(-1,8,128)
print(d.shape)
def nearest(alpha,x): return alpha[np.argmin(np.abs(alpha[None,:]-x[:,None]),axis=1)]
def quant_slot_direct(seg,alpha):
    lmax=np.abs(alpha).max(); pk=np.abs(seg).max()
    best=None
    for s in range(57):
        a=QA[s]
        if a*lmax<pk*0.7: continue
        if a==0: continue
        r=nearest(alpha,seg/a)*a; e=((seg-r)**2).sum()
        if best is None or e<best[0]: best=(e,s)
        if a*lmax>pk*4: break
    return best[0]
def levinson(r,p):
    a=np.zeros(p+1); a[0]=1; e=r[0]; ks=[]
    for i in range(1,p+1):
        acc=r[i]+sum(a[j]*r[i-j] for j in range(1,i))
        k=-acc/e if e>0 else 0
        ks.append(k)
        a2=a.copy()
        for j in range(1,i): a2[j]=a[j]+k*a[i-j]
        a2[i]=k; a=a2; e*=(1-k*k)
    return a,ks
def run(band,level,order=8,use=True):
    x=d[:,band,:].reshape(-1)
    alpha=ALPHA[level]; lmax=np.abs(alpha).max()
    err=0; en=0
    hist=np.zeros(order)
    for f in range(d.shape[0]):
        y=d[f,band,:]
        # LPC on this frame
        r=np.array([np.dot(y[:128-i],y[i:]) for i in range(order+1)])+1e-9
        r[0]*=1.0001
        a,_=levinson(r,order)
        ŷ=np.zeros(128)
        # closed loop per slot
        h=list(hist)  # previous decoded, most recent last
        out=[]
        for slot in range(8):
            seg=y[slot*16:slot*16+16]
            best=None
            for s in range(10,57):
                A=QA[s]
                if A*lmax<np.abs(seg).max()*0.2: continue
                hh=list(h); rec=[]; e=0
                for i in range(16):
                    pred=-sum(a[j]*hh[-j] for j in range(1,order+1))
                    # decoder: y = r - sum c y(prev) ; encoder residual e_i = y_i + sum a_j yhat
                    res=seg[i]-pred
                    q=nearest(alpha,np.array([res/A]))[0]*A
                    yh=q+pred
                    hh.append(yh); rec.append(yh); e+=(seg[i]-yh)**2
                if best is None or e<best[0]: best=(e,rec,hh)
                if A*lmax>np.abs(seg).max()*8: break
            err+=best[0]; h=best[2]
        hist=np.array(h[-order:])
        en+=(y**2).sum()
    return 10*np.log10(en/err)
for band in (0,1,2):
    for lv in (3,4,5):
        direct=0; en=0
        for f in range(d.shape[0]):
            y=d[f,band,:]
            for slot in range(8):
                direct+=quant_slot_direct(y[slot*16:slot*16+16],ALPHA[lv])
            en+=(y**2).sum()
        print("band",band,"L",lv,"direct %.1f dB"%(10*np.log10(en/direct)),"dpcm %.1f dB"%run(band,lv),flush=True)
