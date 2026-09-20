import csv, collections, statistics

series = collections.defaultdict(list)
with open("series.psv") as f:
    for sym, day, close, atr in csv.reader(f, delimiter="|"):
        try: series[sym].append((day, float(close), float(atr)))
        except ValueError: pass
for s in series: series[s].sort()

trips = []
with open("trips.psv") as f:
    for r in csv.reader(f, delimiter="|"):
        if len(r) < 7 or not r[5]: continue
        sym, exit_at, exit_px, qty, gain, entry_at, entry_px = r
        try:
            trips.append(dict(sym=sym, exit_day=exit_at[:10], exit_px=float(exit_px),
                              qty=float(qty), gain=float(gain),
                              entry_day=entry_at[:10], entry_px=float(entry_px)))
        except ValueError: pass

MIN_RATCHET_ATR = 0.25   # strategy.ladder.min_ratchet_atr_fraction

def simulate(t, k):
    """Trail at high_water - k*ATR on daily closes, ratcheting only on moves
    past 0.25 ATR, exactly as strategy.ladder specifies. Returns (exit_price,
    exit_day, stopped) or None when the symbol has no usable series."""
    rows = [r for r in series.get(t["sym"], []) if r[0] >= t["entry_day"]]
    if not rows: return None
    high = t["entry_px"]
    stop = t["entry_px"] - k * rows[0][2]
    for day, close, atr in rows:
        if close <= stop:
            return (stop, day, True)
        if close > high:
            candidate = close - k * atr
            if candidate - stop >= MIN_RATCHET_ATR * atr:
                stop = candidate
            high = close
    return (rows[-1][1], rows[-1][0], False)

print(f"{'k (ATR)':<9}{'trips':>7}{'stopped':>9}{'winners':>9}{'hit %':>8}{'gross DKK':>12}{'avg days':>10}")
print("-"*64)
baseline = None
for k in (2.0, 3.0, 4.0, 5.0):
    tot = wins = stopped = n = 0; days = []
    for t in trips:
        r = simulate(t, k)
        if r is None: continue
        px, day, was_stopped = r
        # Same shares, same entry, price difference in local currency scaled by
        # the realised DKK/local ratio this trip actually printed.
        local_pnl = (px - t["entry_px"]) * t["qty"]
        actual_local = (t["exit_px"] - t["entry_px"]) * t["qty"]
        fx = (t["gain"] / actual_local) if abs(actual_local) > 1e-9 else 1.0
        dkk = local_pnl * fx
        tot += dkk; n += 1
        if dkk > 0: wins += 1
        if was_stopped: stopped += 1
        d = (int(day[:4])*372+int(day[5:7])*31+int(day[8:10])) - \
            (int(t["entry_day"][:4])*372+int(t["entry_day"][5:7])*31+int(t["entry_day"][8:10]))
        days.append(d)
    if baseline is None: baseline = tot
    print(f"{k:<9}{n:>7}{stopped:>9}{wins:>9}{100*wins/n:>7.0f}%{tot:>12,.0f}{statistics.mean(days):>10.1f}")

actual_n = sum(1 for t in trips if simulate(t, 2.0) is not None)
actual = sum(t["gain"] for t in trips if simulate(t, 2.0) is not None)
actual_w = sum(1 for t in trips if simulate(t, 2.0) is not None and t["gain"] > 0)
print("-"*64)
print(f"{'ACTUAL':<9}{actual_n:>7}{'--':>9}{actual_w:>9}{100*actual_w/actual_n:>7.0f}%{actual:>12,.0f}")
print(f"\ntrips with no usable price series: {len(trips)-actual_n} of {len(trips)}")
