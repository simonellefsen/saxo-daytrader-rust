# How an Opportunistic Swing Trader Would Use These Indicators

**Core Philosophy**
An opportunistic swing trader is **selective and flexible** — not robotic.
You scan the market daily for **high-probability setups** only.
You prioritize stocks/sectors that show **multiple indicator confluences** + clean price action + volume confirmation + optional catalysts (earnings, news, sector rotation).
Ignore marginal or low-confluence signals.
Goal: asymmetric risk/reward (minimum 1:2 risk-reward ratio) on every trade.
Hold period target: 5–30 calendar days (week-to-month window).

**Timeframe**: Daily charts only (perfect for 1-week to 1-month swings).
**Markets**: Stocks, ETFs, or indices (focus on liquid names with good volume).

## Step-by-Step Trading Workflow

### Step 1: Establish Trend Bias (Filter – Never Fight the Market)
Use **Moving Averages** + **MACD** as your trend filter:
- **Bullish bias (longs only)**:
  - Price trading **above** rising 50-day and 200-day EMA/SMA
  - MACD line above signal line **and** above zero line (or showing bullish structure)
- **Bearish bias (shorts only)**:
  - Price trading **below** falling 50-day and 200-day EMA/SMA
  - MACD line below signal line **and** below zero line
- **No trade zone**: When price is chopping between MAs or MACD is flat/oscillating around zero.

**Rule**: Only take long setups in confirmed uptrends. Only take short setups in confirmed downtrends.

### Step 2: Wait for High-Probability Pullback or Setup
In your chosen trend direction, look for a **pullback** that meets **at least 3 of the following**:
- **RSI (14)**:
  - Longs: RSI dips to 30–45 zone then starts turning up (oversold in the context of uptrend)
  - Shorts: RSI rises to 55–70 zone then starts turning down
- **MACD (12,26,9)**:
  - Longs: MACD line crosses **above** signal line **or** histogram flips from negative to positive and expands
  - Shorts: MACD line crosses **below** signal line **or** histogram flips from positive to negative and expands
- **Bollinger Bands (20,2)**:
  - Longs: Price touches or rides the lower band (in uptrend)
  - Shorts: Price touches or rides the upper band (in downtrend)
  - Bonus: Bollinger Squeeze (bands narrow) + breakout in trend direction
- **Stochastic (14,3,3)** (optional confirmation):
  - Longs: %K crosses %D upward from below 20–30
  - Shorts: %K crosses %D downward from above 70–80
- **Volume / OBV**: Rising volume on the reversal candle + OBV confirming the move (no divergence)

**Confluence Rule**: Minimum **3 indicators** aligning + volume support = trade candidate.

### Step 3: Confirm Entry with Price Action
- Wait for a **bullish candlestick pattern** (hammer, engulfing, pin bar) at support for longs
- Wait for a **bearish candlestick pattern** at resistance for shorts
- Enter on the **close** of the confirmation candle (or next day open if using limit orders)

**Entry Checklist (copy-paste into your app rules)**:
- [ ] Trend bias confirmed (MA + MACD)
- [ ] At least 3 oscillators aligning
- [ ] Volume/OBV supportive
- [ ] Clean candlestick reversal at key level
- [ ] No major news/events that could cause gap risk

### Step 4: Risk Management & Position Sizing
- **Stop-loss**: Always placed
  - Below recent swing low (longs) or above swing high (shorts)
  - Or 1.5–2× ATR(14) below entry
- **Risk per trade**: Maximum **1–2%** of total account equity
- **Position size formula**:
  `Position size = (Account risk %) / (Entry price – Stop price)`
- **Profit target**: Minimum **1:2** risk-reward
  - Primary target: Next major resistance/support or opposite indicator signal
  - Secondary target: Measured move or previous swing high/low

### Step 5: Trade Management (Opportunistic Style)
- **Trail stops**: Move stop to breakeven once +1R profit is reached
- **Partial profits**: Take 50% off at 1:1 or 1:2, let the rest run with trailing stop
- **Exit signals** (take profit or cut early):
  - MACD bearish/bullish crossover in opposite direction
  - RSI reaches extreme (≥70 longs / ≤30 shorts)
  - Price hits opposite Bollinger Band
  - Loss of volume momentum or OBV divergence appears
- Hold 5–30 days max. Cut any trade that stalls for >10 days with no progress.

### Step 6: End-of-Day Review & Continuous Learning (Automated by Trading App)
**Timing**: Automatically triggered **after US market close** (4:30 PM ET / 22:30 CEST or later, once all data is finalized).

The app performs a structured reflection on the day’s activity and builds institutional knowledge over time:

#### Daily Analysis
- Review **all closed trades** and **open positions** from the session
- Score each trade: What went well (e.g., “MACD + RSI confluence in strong uptrend produced 1:3 R:R”) vs. what went bad (e.g., “Entered without volume confirmation → stopped out on fakeout”)
- Calculate key metrics: win rate, average risk-reward, largest winner/loser, adherence to rules
- Extract **actionable insights** (tagged by market condition: trending, ranging, high-volatility, news-driven, etc.)
- Log examples: indicator performance, common failure patterns, best setups of the day

#### Weekly Cadence (every Sunday night)
- Aggregate the last 5–7 daily logs into a **weekly performance summary**
- Identify recurring patterns (e.g., “RSI oversold signals work 75% in uptrends but only 40% in chop”)
- Highlight strategy strengths/weaknesses for the week
- Generate **one high-level adjustment recommendation** for the following week

#### Monthly Cadence (last day of each month)
- Deep-dive review of the entire month
- Compare against previous months (year-over-year if available)
- Evaluate overall edge: win rate, profit factor, drawdown, expectancy
- Archive lessons into a long-term knowledge base
- Suggest **major rule tweaks** if performance drifts (e.g., increase minimum confluence to 4 indicators during low-volatility periods)

#### Application of Learnings (Next Trading Day)
- The app **automatically loads** the most relevant daily/weekly/monthly insights into the morning scan/filter process
- Example prompts the app shows:
  - “Today’s market is showing choppy conditions → increase RSI + Stochastic confluence requirement”
  - “Recent learnings: Bollinger Band squeezes have 82% success rate this month → prioritize these setups”
  - “Avoid entries without OBV confirmation based on last week’s 3 failed trades”
- Insights appear as **smart filters or alerts** in your daily watchlist
- Knowledge base is searchable and grows with every trading day

This turns the app into a **self-improving trading journal** that compounds your edge over time.

## Example Long Setup (Daily Chart)
1. Stock in clear uptrend (price > rising 50/200 EMA, MACD above zero)
2. Pulls back to 20-day EMA / lower Bollinger Band
3. RSI drops to 35 then turns up
4. MACD shows bullish crossover + expanding histogram
5. Bullish engulfing candle on increasing volume
→ **Enter long** at close. Stop below recent low. Target = prior high or RSI 70 zone.

**Reverse the entire setup for shorts.**

## Final Notes for Your Trading App
- **Daily routine**: Scan → Filter by trend bias → Check confluence → Mark watchlist → Execute → End-of-Day Review
- **Maximum positions**: 4–8 open trades at once (diversified)
- **Journal every trade**: Note which indicators aligned and outcome (fully automated via Step 6)
- **Backtest first**: Test this exact rule set on your favorite assets before live trading
- **Market conditions matter**: In strong trends lean more on MACD/MA. In ranging/choppy markets lean more on RSI + Stochastic + Bollinger. Let the learning system adapt rules dynamically.

This framework turns the popular indicators (MACD, RSI, MAs, Bollinger, Stochastic, Volume) into a repeatable, opportunistic swing system tailored for the 1-week to 1-month holding window **with built-in intelligence that improves every single day**.

Save this file as `swing-trading-rules.md` and import/reference it directly in your trading app or journal.
