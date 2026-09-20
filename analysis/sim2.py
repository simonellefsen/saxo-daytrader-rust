import csv, collections, statistics
exec(open("sim.py").read().split("MIN_RATCHET_ATR")[0])   # reuse loaders

def run(label, exit_fn):
    tot=wins=n=0; days=[]
    for t in trips:
        rows=[r for r in series.get(t["sym"],[]) if r[0]>=t["entry_day"]]
        if not rows: continue
        px,day = exit_fn(t,rows)
        local=(px-t["entry_px"])*t["qty"]
        act=(t["exit_px"]-t["entry_px"])*t["qty"]
        fx=(t["gain"]/act) if abs(act)>1e-9 else 1.0
        d=local*fx; tot+=d; n+=1
        if d>0: wins+=1
        days.append((int(day[:4])*372+int(day[5:7])*31+int(day[8:10]))-
                    (int(t["entry_day"][:4])*372+int(t["entry_day"][5:7])*31+int(t["entry_day"][8:10])))
    print(f"{label:<34}{n:>6}{wins:>9}{100*wins/n:>7.0f}%{tot:>12,.0f}{statistics.mean(days):>10.1f}")

def trail(k, floor=None):
    def f(t, rows):
        high=t["entry_px"]; stop=t["entry_px"]-k*rows[0][2]
        if floor: stop=max(stop, t["entry_px"]*(1-floor))
        for day,close,atr in rows:
            if close<=stop: return (stop,day)
            if close>high:
                c=close-k*atr
                if c-stop>=0.25*atr: stop=c
                high=close
        return (rows[-1][1],rows[-1][0])
    return f

def hold(sessions, disaster):
    """Hold a fixed horizon; only a hard disaster stop can exit early."""
    def f(t, rows):
        floor=t["entry_px"]*(1-disaster)
        for i,(day,close,atr) in enumerate(rows):
            if close<=floor: return (floor,day)
            if i+1>=sessions: return (close,day)
        return (rows[-1][1],rows[-1][0])
    return f

print(f"{'rule':<34}{'trips':>6}{'winners':>9}{'hit %':>8}{'gross DKK':>12}{'avg days':>10}")
print("-"*79)
for k in (2.0,3.0,4.0):
    run(f"trailing stop {k} ATR", trail(k))
print("-"*79)
for s in (10,20,30,40):
    run(f"hold {s} sessions, -15% disaster", hold(s,0.15))
print("-"*79)
run("hold 30 sessions, -10% disaster", hold(30,0.10))
run("hold 30 sessions, -20% disaster", hold(30,0.20))
print("-"*79)
actual=sum(t["gain"] for t in trips if series.get(t["sym"]))
aw=sum(1 for t in trips if series.get(t["sym"]) and t["gain"]>0)
an=sum(1 for t in trips if series.get(t["sym"]))
print(f"{'ACTUAL (2 ATR, intraday)':<34}{an:>6}{aw:>9}{100*aw/an:>7.0f}%{actual:>12,.0f}")

print("\n=== how far down does 'shorter is better' go? ===")
print(f"{'rule':<34}{'trips':>6}{'winners':>9}{'hit %':>8}{'gross DKK':>12}{'avg days':>10}")
print("-"*79)
for s in (1,2,3,5,8,10,15):
    run(f"hold {s} sessions, -15% disaster", hold(s,0.15))
