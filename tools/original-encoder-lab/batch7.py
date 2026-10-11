import lab
specs=[]
for st,rate,kb in (("es",48000,1509),("hra",96000,4224),("hra",48000,4608),("ma",48000,0)):
    for lay in ("6.1d","6.0d"):
        specs.append(dict(name="l_%s_%d_%s"%(st,rate,lay),group="es51",dest="bd",stream=st,rate=rate,layout=lay,kbps=kb,use51=False,legacy_matrix=True))
lab.run_many(specs,timeout=120)
lab.Engine.close()
