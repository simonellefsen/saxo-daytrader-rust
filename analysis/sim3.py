import csv, collections, statistics
exec(open("sim.py").read().split("MIN_RATCHET_ATR")[0])

# Pure gross price return in percent -- no DKK, no fx, no cost scaling.
# Isolates whether the entries themselves go up, independent of my conversion.
print("Gross price return from entry, no costs, no fx  (n=59)")
print(f"{'horizon':<16}{'mean %':>9}{'median %':>10}{'% positive':>12}")
print("-"*47)
for s in (1,3,5,8,10,15,20,30,40):
    rets=[]
    for t in trips:
        rows=[r for r in series.get(t["sym"],[]) if r[0]>=t["entry_day"]]
        if not rows: continue
        close=rows[min(s,len(rows))-1][1]
        rets.append(100*(close/t["entry_px"]-1))
    print(f"hold {s:<11}{statistics.mean(rets):>9.2f}{statistics.median(rets):>10.2f}"
          f"{100*sum(1 for r in rets if r>0)/len(rets):>11.0f}%")
