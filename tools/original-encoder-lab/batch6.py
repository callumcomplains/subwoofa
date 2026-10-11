import lab
specs=[]
for st,rate,kb in (("es",48000,1509),("hra",96000,4224),("hra",48000,4608),("ma",48000,0),("core96",96000,1509)):
    for lay in ("6.1d","6.0d","6.1m","6.0m"):
        for u51 in (False,True):
            nm="v_%s_%d_%s_%s"%(st,rate,lay,"dm" if u51 else "nodm")
            specs.append(dict(name=nm,group="es51",dest="bd",stream=st,rate=rate,layout=lay,kbps=kb,use51=u51))
lab.run_many(specs,timeout=120)
lab.Engine.close()
