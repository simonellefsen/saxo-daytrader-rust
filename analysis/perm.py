import random, statistics, math
exec(open("sep.py").read().split("W=[r for r")[0])

FEATS=["rsi14","atr_pct","macd_hist","confluence","reward_risk",
       "vs_sma20","vs_sma50","vs_sma200","headroom_pct","notional"]

def max_abs_d(labels):
    best=0.0; who=None
    for k in FEATS:
        a=[r[k] for r,l in zip(rows,labels) if l and r.get(k) is not None]
        b=[r[k] for r,l in zip(rows,labels) if not l and r.get(k) is not None]
        if len(a)<3 or len(b)<3: continue
        va,vb=statistics.pvariance(a),statistics.pvariance(b)
        sp=math.sqrt(((len(a)-1)*va+(len(b)-1)*vb)/max(len(a)+len(b)-2,1))
        if sp<=0: continue
        d=abs((statistics.mean(a)-statistics.mean(b))/sp)
        if d>best: best,who=d,k
    return best,who

actual=[r["win"] for r in rows]
obs,obs_feat=max_abs_d(actual)

random.seed(7)
null=[]
for _ in range(20000):
    sh=actual[:]; random.shuffle(sh)
    null.append(max_abs_d(sh)[0])
null.sort()
p=sum(1 for x in null if x>=obs)/len(null)

print("PERMUTATION TEST  (20,000 shuffles of the win/lose label)")
print(f"  strongest real feature      : {obs_feat}  |d| = {obs:.3f}")
print(f"  median |d| under pure chance: {statistics.median(null):.3f}")
print(f"  95th percentile under chance: {null[int(.95*len(null))]:.3f}")
print(f"  p-value (family-wise)       : {p:.3f}")
print(f"\n  => {'SIGNIFICANT' if p<0.05 else 'NOT distinguishable from chance'}")

# day-of-week, same treatment
print("\nDAY-OF-WEEK, permutation on the Mon+Tue vs Wed-Fri split")
early=[r for r in rows if r["dow"] in ("Mon","Tue")]
late =[r for r in rows if r["dow"] in ("Wed","Thu","Fri")]
obs_gap=(sum(r["win"] for r in early)/len(early))-(sum(r["win"] for r in late)/len(late))
labels=[r["win"] for r in rows]; idx_early=[i for i,r in enumerate(rows) if r["dow"] in ("Mon","Tue")]
cnt=0
for _ in range(20000):
    sh=labels[:]; random.shuffle(sh)
    e=[sh[i] for i in idx_early]; l=[sh[i] for i in range(len(rows)) if i not in set(idx_early)]
    if (sum(e)/len(e))-(sum(l)/len(l))>=obs_gap: cnt+=1
print(f"  Mon+Tue hit rate {100*sum(r['win'] for r in early)/len(early):.0f}% (n={len(early)})"
      f"  vs Wed-Fri {100*sum(r['win'] for r in late)/len(late):.0f}% (n={len(late)})")
print(f"  one-sided p (single pre-chosen split, NOT corrected for the 4 other cuts I looked at): {cnt/20000:.3f}")
