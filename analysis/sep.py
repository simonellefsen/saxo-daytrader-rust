import csv, statistics, math

COLS = ["symbol","entry_at","exit_at","gain","entry_px","qty","rsi14","atr14","sma20",
        "sma50","sma200","macd_hist","confluence","reward_risk","support","resistance",
        "trend_bias","sentiment","asset_type","close","min_confluences"]

rows=[]
with open("features.psv") as f:
    for r in csv.reader(f, delimiter="|"):
        if len(r)<len(COLS): continue
        d=dict(zip(COLS,r))
        def num(k):
            try: return float(d[k])
            except (ValueError,KeyError): return None
        rec=dict(symbol=d["symbol"], win=float(d["gain"])>0, gain=float(d["gain"]),
                 entry_at=d["entry_at"], exchange=d["symbol"].split(":")[-1],
                 trend_bias=d["trend_bias"] or "n/a", sentiment=d["sentiment"] or "n/a")
        px=num("entry_px")
        for k in ("rsi14","atr14","sma20","sma50","sma200","macd_hist","confluence",
                  "reward_risk","support","resistance","close"):
            rec[k]=num(k)
        # derived, decision-time only
        rec["atr_pct"] = 100*rec["atr14"]/px if rec["atr14"] and px else None
        for sma in ("sma20","sma50","sma200"):
            rec["vs_"+sma] = 100*(px/rec[sma]-1) if rec[sma] else None
        rec["notional"] = px*num("qty") if px and num("qty") else None
        rec["headroom_pct"] = 100*(rec["resistance"]/px-1) if rec["resistance"] and px else None
        rec["dow"] = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"][
            (lambda y,m,dd: __import__("datetime").date(y,m,dd).weekday())(
                int(d["entry_at"][:4]),int(d["entry_at"][5:7]),int(d["entry_at"][8:10]))]
        rec["region"] = "US" if rec["exchange"].startswith("xn") else "EU"
        rows.append(rec)

W=[r for r in rows if r["win"]]; L=[r for r in rows if not r["win"]]
print(f"n={len(rows)}   winners={len(W)}   losers={len(L)}\n")

def cohen(a,b):
    if len(a)<2 or len(b)<2: return None
    va,vb=statistics.pvariance(a),statistics.pvariance(b)
    sp=math.sqrt(((len(a)-1)*va+(len(b)-1)*vb)/max(len(a)+len(b)-2,1))
    return (statistics.mean(a)-statistics.mean(b))/sp if sp>0 else None

print("NUMERIC FEATURES  (winner mean vs loser mean)")
print(f"{'feature':<16}{'win n':>6}{'win mean':>11}{'lose n':>7}{'lose mean':>11}{'effect d':>10}")
print("-"*61)
out=[]
for k in ("rsi14","atr_pct","macd_hist","confluence","reward_risk",
          "vs_sma20","vs_sma50","vs_sma200","headroom_pct","notional"):
    a=[r[k] for r in W if r.get(k) is not None]
    b=[r[k] for r in L if r.get(k) is not None]
    if len(a)<3 or len(b)<3: continue
    d=cohen(a,b)
    out.append((abs(d or 0),k,len(a),statistics.mean(a),len(b),statistics.mean(b),d))
for _,k,na,ma,nb,mb,d in sorted(out, reverse=True):
    print(f"{k:<16}{na:>6}{ma:>11.2f}{nb:>7}{mb:>11.2f}{(d if d is not None else 0):>10.2f}")

print("\nCATEGORICAL FEATURES")
for k in ("region","trend_bias","sentiment","dow","exchange"):
    groups={}
    for r in rows: groups.setdefault(r[k],[]).append(r)
    items=[(g,v) for g,v in groups.items() if len(v)>=4]
    if not items: continue
    print(f"\n  {k}")
    for g,v in sorted(items, key=lambda x:-len(x[1])):
        w=sum(1 for r in v if r["win"]); net=sum(r["gain"] for r in v)
        print(f"    {g:<12} n={len(v):<4} win={w:<3} {100*w/len(v):>3.0f}%   net {net:>10,.0f} DKK")
