import collections, datetime
exec(open("sep.py").read().split("W=[r for r")[0])
def wk(r):
    d=r["entry_at"][:10]
    return datetime.date(int(d[:4]),int(d[5:7]),int(d[8:10])).isocalendar()[1]
for r in rows: r["wk"]=wk(r)

def split(sub):
    e=[r for r in sub if r["dow"] in ("Mon","Tue")]
    l=[r for r in sub if r["dow"] in ("Wed","Thu","Fri")]
    if not e or not l: return None
    return (len(e),100*sum(r["win"] for r in e)/len(e),
            len(l),100*sum(r["win"] for r in l)/len(l))

print("Leave-one-week-out: does the Mon+Tue edge survive dropping any single week?\n")
print(f"{'dropped week':<14}{'Mon+Tue n':>11}{'hit %':>8}{'Wed-Fri n':>11}{'hit %':>8}{'gap':>8}")
print("-"*60)
base=split(rows)
print(f"{'none':<14}{base[0]:>11}{base[1]:>8.0f}{base[2]:>11}{base[3]:>8.0f}{base[1]-base[3]:>8.0f}")
worst=None
for w in sorted({r["wk"] for r in rows}):
    s=split([r for r in rows if r["wk"]!=w])
    if not s: continue
    gap=s[1]-s[3]
    if worst is None or gap<worst[1]: worst=(w,gap,s)
    if w in (32,37,36,35):
        print(f"{w:<14}{s[0]:>11}{s[1]:>8.0f}{s[2]:>11}{s[3]:>8.0f}{gap:>8.0f}")
w,gap,s=worst
print(f"\nmost fragile: dropping week {w} takes the gap from {base[1]-base[3]:.0f}pp to {gap:.0f}pp")
print(f"  -> Mon+Tue {s[1]:.0f}% (n={s[0]})  vs  Wed-Fri {s[3]:.0f}% (n={s[2]})")
