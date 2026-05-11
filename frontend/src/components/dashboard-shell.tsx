"use client";

import { useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";

import useSWR, { mutate } from "swr";

import { apiFetch, getFetcher, postAction } from "@/lib/api";
import { formatDkk, formatLocalMoney, formatNumber, formatPercent, formatTimestamp, formatTimestampPrecise, signedClass, toYahooFinanceUrl } from "@/lib/format";
import type {
  AssetLadderHistoryResponse,
  DecisionHistoryResponse,
  DecisionResponse,
  ExecutionResponse,
  MarketResponse,
  OverviewResponse,
  PerformanceResponse,
  PositionsResponse,
  PromptsResponse,
  SaxoAuthStatus,
  SchedulerResponse,
  StrategyJournalResponse,
  WatchlistCategory,
  WatchlistsResponse,
} from "@/lib/types";
import { LadderVisualizer } from "@/components/ladder-visualizer";
import { LineChart } from "@/components/line-chart";
import { Sparkline } from "@/components/sparkline";

type TabKey = "portfolio" | "performance" | "market" | "watchlist" | "decision" | "prompts" | "journal" | "execution";

type AuthSession = {
  authenticated: boolean;
  user: {
    email: string;
    name: string;
  } | null;
};

type ReconciliationAction = {
  path: string;
  title: string;
  buttonLabel: string;
  confirmLabel: string;
  direction: string;
  details: string[];
  warning: string;
};

const TAB_OPTIONS: Array<{ key: TabKey; label: string }> = [
  { key: "portfolio", label: "Portfolio" },
  { key: "performance", label: "Performance" },
  { key: "market", label: "Market Status" },
  { key: "watchlist", label: "Watchlist" },
  { key: "decision", label: "Decision Report" },
  { key: "prompts", label: "AI Prompts" },
  { key: "journal", label: "EOD Diary" },
  { key: "execution", label: "Execution" },
];

const PERFORMANCE_RANGES = ["1D", "1W", "1M", "3M", "YTD", "1Y", "ALL"] as const;

const PORTFOLIO_COLUMN_HELP: Record<string, string> = {
  Symbol: "Trading symbol. Click to open the instrument on Yahoo Finance.",
  Decision: "Latest per-symbol decision sentiment from the most recent xAI analysis report.",
  "Ladder Status": "Current ladder strategy state for the symbol.",
  Trend: "Short intraday sparkline from the recent chart window.",
  Qty: "Current broker-aligned quantity held.",
  Currency: "Trading currency of the instrument.",
  "Paid Price": "Average price paid per unit in the instrument currency.",
  "Current Price": "Latest polled market price in the instrument currency.",
  "Cost Basis DKK": "Current remaining acquisition cost in DKK for the held quantity.",
  "Market Value DKK": "Current position value in DKK using the latest price and FX rate.",
  "Unrealised P/L DKK": "Unrealised profit or loss in DKK before tax.",
  "FX Gain/Loss DKK": "Part of unrealised P/L caused by FX movement since purchase.",
  "Daily P/L DKK": "Change in DKK since the 06:00 Copenhagen intraday baseline.",
  Allocation: "Share of total portfolio value currently allocated to this position.",
  "Quote Updated": "Timestamp of the latest stored quote used for this row.",
};

function decisionTone(sentiment: unknown): "good" | "warn" | "bad" | "neutral" {
  const normalized = String(sentiment ?? "").toUpperCase();
  if (normalized === "BUY" || normalized === "OVERWEIGHT") return "good";
  if (normalized === "SELL") return "bad";
  if (normalized === "UNDERWEIGHT") return "warn";
  return "neutral";
}

function listPreview(value: unknown): string {
  if (!Array.isArray(value) || value.length === 0) {
    return "n/a";
  }
  return value.slice(0, 3).map((item) => String(item)).join("; ");
}

function stringList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.map((item) => String(item)).filter(Boolean);
}

function cashBufferDecimals(value: number): number {
  return value > 0 && value < 5 ? 1 : 0;
}

function normalizeCashBufferPct(value: number): number {
  if (!Number.isFinite(value) || value <= 0) {
    return 0;
  }
  return value < 5 ? Math.round(value * 10) / 10 : Math.round(value);
}

function formatCashBufferPct(value: number): string {
  if (!Number.isFinite(value) || value <= 0) {
    return "disabled";
  }
  return `${formatNumber(value, cashBufferDecimals(value))}%`;
}

function DecisionCell({ decision, nowMs }: { decision: Record<string, any> | null | undefined; nowMs: number }) {
  const cellRef = useRef<HTMLSpanElement | null>(null);
  const tooltipRef = useRef<HTMLSpanElement | null>(null);
  const [tooltipStyle, setTooltipStyle] = useState<CSSProperties | undefined>();
  const [tooltipOpen, setTooltipOpen] = useState(false);

  function updateTooltipPosition() {
    const cell = cellRef.current;
    const tooltip = tooltipRef.current;
    if (!cell || !tooltip) {
      return;
    }
    const cellRect = cell.getBoundingClientRect();
    const gap = 10;
    const margin = 12;
    const width = Math.min(420, Math.max(window.innerWidth - margin * 2, 280));
    const tooltipHeight = tooltip.getBoundingClientRect().height;
    const rightAlignedLeft = cellRect.right + gap;
    const leftAlignedLeft = cellRect.left - width - gap;
    const left =
      rightAlignedLeft + width <= window.innerWidth - margin
        ? rightAlignedLeft
        : leftAlignedLeft >= margin
          ? leftAlignedLeft
          : window.innerWidth - width - margin;
    const top = Math.min(
      Math.max(cellRect.top, margin),
      Math.max(window.innerHeight - tooltipHeight - margin, margin),
    );
    setTooltipStyle({
      left,
      top,
      width,
    });
  }

  useEffect(() => {
    if (!tooltipOpen) {
      return;
    }
    const frame = window.requestAnimationFrame(updateTooltipPosition);
    window.addEventListener("resize", updateTooltipPosition);
    window.addEventListener("scroll", updateTooltipPosition, true);
    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", updateTooltipPosition);
      window.removeEventListener("scroll", updateTooltipPosition, true);
    };
  }, [tooltipOpen]);

  function openTooltip() {
    setTooltipStyle(undefined);
    setTooltipOpen(true);
  }

  if (!decision) {
    return <span className="muted">n/a</span>;
  }
  const sentiment = String(decision.sentiment ?? "n/a").toUpperCase();
  const age = timeAgo(String(decision.created_at ?? ""), nowMs);
  const action = decision.action ? String(decision.action) : null;
  const priority = decision.priority ? String(decision.priority) : null;

  return (
    <span
      className="decision-cell"
      ref={cellRef}
      onBlur={() => setTooltipOpen(false)}
      onFocus={openTooltip}
      onMouseEnter={openTooltip}
      onMouseLeave={() => setTooltipOpen(false)}
    >
      <span className={`decision-chip ${decisionTone(sentiment)}`}>{sentiment}</span>
      <span className="decision-age">{age}</span>
      {tooltipOpen && typeof document !== "undefined"
        ? createPortal(
            <span
              className="decision-tooltip visible"
              ref={tooltipRef}
              role="tooltip"
              style={tooltipStyle ?? { left: -9999, top: -9999 }}
            >
              <strong>
                {sentiment}
                {action ? ` · ${action}` : ""}
              </strong>
              <span>
                {priority ? `Priority ${priority} · ` : ""}
                Confidence {formatNumber(decision.target_confidence ?? decision.confidence, 0)}
              </span>
              <span>Report #{String(decision.report_id ?? "n/a")} · {age}</span>
              <span>{String(decision.target_rationale ?? decision.rationale ?? "No rationale recorded.")}</span>
              <span>Catalysts: {listPreview(decision.catalysts)}</span>
              <span>Risks: {listPreview(decision.risk_notes)}</span>
            </span>,
            document.body,
          )
        : null}
    </span>
  );
}

function PortfolioRow({
  row,
  onOpen,
  nowMs,
}: {
  row: Record<string, any>;
  onOpen: (symbol: string) => void;
  nowMs: number;
}) {
  const symbol = String(row.symbol ?? "");
  const sparkline = useSWR<AssetLadderHistoryResponse>(
    `/api/ladder-chart/${encodeURIComponent(symbol)}?range_key=1H`,
    getFetcher,
    { refreshInterval: 120_000 },
  );
  const sparkValues = (sparkline.data?.chart?.points ?? []).map((point) => Number(point.close ?? 0)).filter((value) => Number.isFinite(value) && value > 0);
  const positive = sparkValues.length > 1 ? sparkValues[sparkValues.length - 1] >= sparkValues[0] : Number(row.daily_pnl_dkk ?? 0) >= 0;
  const filledRungs = Number(row.ladder_status?.filled_entry_rungs ?? 0);
  const totalRungs = Number(row.ladder_status?.total_entry_rungs ?? 0);
  const rungProgressText = totalRungs > 0 ? `${formatNumber(filledRungs, 0)}/${formatNumber(totalRungs, 0)} rungs filled` : null;

  return (
    <tr className="clickable-row" key={symbol} onClick={() => onOpen(symbol)}>
      <td title={String(row.instrument_name ?? symbol)}>
        <a
          href={toYahooFinanceUrl(symbol)}
          target="_blank"
          rel="noreferrer"
          title={String(row.instrument_name ?? symbol)}
          onClick={(event) => event.stopPropagation()}
        >
          {symbol}
        </a>
      </td>
      <td>
        <DecisionCell decision={row.decision} nowMs={nowMs} />
      </td>
      <td>
        <div className="ladder-status-cell">
          <span className={`status-chip ${row.ladder_status?.trailing ? "good" : "neutral"}`}>
            {String(row.ladder_status?.text ?? "idle")}
          </span>
          {rungProgressText ? <small className="ladder-rung-text">{rungProgressText}</small> : null}
          {Number(row.ladder_status?.progress_pct ?? 0) > 0 ? (
            <div className="ladder-progress">
              <div className="ladder-progress-bar" style={{ width: `${Math.max(6, Math.round(Number(row.ladder_status?.progress_pct ?? 0) * 100))}%` }} />
            </div>
          ) : null}
        </div>
      </td>
      <td>
        <button
          className="sparkline-button"
          type="button"
          onClick={(event) => {
            event.stopPropagation();
            onOpen(symbol);
          }}
          aria-label={`Open ladder visualizer for ${symbol}`}
        >
          <Sparkline values={sparkValues} positive={positive} />
        </button>
      </td>
      <td>{formatNumber(row.quantity, 0)}</td>
      <td>{String(row.currency ?? "")}</td>
      <td>
        {row.paid_price_local === null || row.paid_price_local === undefined
          ? "n/a"
          : formatLocalMoney(row.paid_price_local, row.currency)}
      </td>
      <td>{formatLocalMoney(row.current_price_local, row.currency)}</td>
      <td>{formatDkk(row.cost_basis_dkk)}</td>
      <td>{formatDkk(row.market_value_dkk)}</td>
      <td className={signedClass(row.unrealised_pnl_dkk)}>{formatDkk(row.unrealised_pnl_dkk)}</td>
      <td className={signedClass(row.fx_unrealised_pnl_dkk)}>
        {row.fx_unrealised_pnl_dkk === null || row.fx_unrealised_pnl_dkk === undefined
          ? "n/a"
          : formatDkk(row.fx_unrealised_pnl_dkk)}
      </td>
      <td className={signedClass(row.daily_pnl_dkk)}>{formatDkk(row.daily_pnl_dkk)}</td>
      <td>{formatPercent(row.allocation_pct)}</td>
      <td>{formatTimestamp(row.latest_quote_updated_at)}</td>
    </tr>
  );
}

function metricSubvalue(value: unknown, formatter: (value: unknown) => string) {
  if (value === null || value === undefined) {
    return "n/a";
  }
  return formatter(value);
}

function userInitials(session: AuthSession | undefined): string {
  const source = session?.user?.name || session?.user?.email || "?";
  const words = source
    .replace(/@.*/, "")
    .split(/[\s._-]+/)
    .filter(Boolean);
  return `${words[0]?.[0] ?? "?"}${words[1]?.[0] ?? ""}`.toUpperCase();
}

function timeAgo(value: string | null, nowMs: number): string {
  if (!value) {
    return "n/a";
  }
  const parsed = new Date(value).getTime();
  if (!Number.isFinite(parsed)) {
    return formatTimestampPrecise(value);
  }
  const seconds = Math.max(Math.floor((nowMs - parsed) / 1000), 0);
  if (seconds < 5) {
    return "just now";
  }
  if (seconds < 60) {
    return `${seconds} seconds ago`;
  }
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) {
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }
  return formatTimestampPrecise(value);
}

function saxoTone(status: SaxoAuthStatus | undefined): "good" | "warn" | "bad" {
  if (!status || status.needs_reauth || status.status === "missing_session" || status.status === "session_error") {
    return "bad";
  }
  if (status.status === "expiring_soon" || status.status === "refresh_available" || !status.connected) {
    return "warn";
  }
  return "good";
}

function saxoStatusTitle(status: SaxoAuthStatus | undefined): string {
  if (!status) {
    return "Saxo status is loading.";
  }
  const expires = status.expires_in_minutes === null || status.expires_in_minutes === undefined ? "n/a" : `${status.expires_in_minutes} min`;
  const refreshExpires =
    status.refresh_expires_in_minutes === null || status.refresh_expires_in_minutes === undefined
      ? "n/a"
      : `${status.refresh_expires_in_minutes} min`;
  return [
    status.status_text ?? status.status,
    `Environment: ${String(status.environment ?? "n/a").toUpperCase()}`,
    `Access token valid: ${status.token_valid ? "yes" : "no"}`,
    `Expires in: ${expires}`,
    `Refresh token valid: ${status.refresh_token_valid ? "yes" : "no"}`,
    `Refresh expires in: ${refreshExpires}`,
    `Last refreshed: ${formatTimestamp(status.last_refreshed_at)}`,
  ].join("\n");
}

function SaxoStatusPill({
  status,
  onReauth,
  pending,
}: {
  status: SaxoAuthStatus | undefined;
  onReauth: () => void;
  pending: boolean;
}) {
  const tone = saxoTone(status);
  const env = String(status?.environment ?? "n/a").toUpperCase();
  const expires = status?.expires_in_minutes === null || status?.expires_in_minutes === undefined ? "n/a" : `${status.expires_in_minutes} min`;
  const statusCopy =
    status?.connected
      ? `Connected, token expires in ${expires}`
      : status?.needs_reauth
        ? "Saxo disconnected"
        : `Saxo token refreshable, expires in ${expires}`;
  return (
    <div className={`saxo-status-pill ${tone}`} title={saxoStatusTitle(status)}>
      <span className="status-dot" aria-hidden="true" />
      <span className={`env-badge ${env.toLowerCase()}`}>{env}</span>
      <span className="status-copy">{statusCopy}</span>
      {status?.needs_reauth ? (
        <button className="inline-action" type="button" onClick={onReauth} disabled={pending}>
          {pending ? <span className="button-spinner" aria-hidden="true" /> : null}
          Re-auth
        </button>
      ) : null}
    </div>
  );
}

function ActionButton({
  className,
  disabled,
  loading,
  onClick,
  children,
}: {
  className: string;
  disabled: boolean;
  loading: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button className={className} type="button" disabled={disabled} onClick={onClick}>
      {loading ? <span className="button-spinner" aria-hidden="true" /> : null}
      {children}
    </button>
  );
}

function decisionFriendlyMessage(decision: Record<string, any> | null, saxoAuthStatus: SaxoAuthStatus | undefined): string | null {
  const status = String(decision?.status ?? "");
  const strategyStatus = String(decision?.report_json?.strategy_plan?.status ?? "");
  if (status === "xai_fallback") {
    const error = String(decision?.error_text ?? "").trim();
    return `xAI request did not complete in time; using deterministic fallback strategy output.${error ? ` Original error: ${error}` : ""}`;
  }
  if (status === "failed") {
    if (strategyStatus === "ok") {
      return "xAI request failed, but deterministic strategy output was generated. Treat the model narrative as fallback-quality and review the details before execution.";
    }
    const saxoCopy = saxoAuthStatus?.connected
      ? "Current Saxo status is connected; inspect the report error/logs for the original failure."
      : "Saxo is not currently connected; renew the session before relying on the next automatic cycle.";
    return `xAI decision report failed. ${saxoCopy}`;
  }
  if (status === "no_scored_candidates" || strategyStatus === "no_scored_candidates") {
    return "No tradable candidates were found. The system will wait for the next analysis window and current cash/market constraints.";
  }
  if (strategyStatus === "saxo_session_error") {
    return "This report was generated while Saxo session data was unavailable. The top-bar Saxo indicator shows the current connection state.";
  }
  return null;
}

function decisionStatusLabel(decision: Record<string, any> | null | undefined): string {
  const status = String(decision?.status ?? "n/a");
  const strategyStatus = String(decision?.report_json?.strategy_plan?.status ?? "");
  if (status === "xai_fallback" || (status === "failed" && strategyStatus === "ok")) {
    return "fallback";
  }
  return status;
}

function isTodayTimestamp(value: unknown): boolean {
  if (!value || typeof value !== "string") {
    return false;
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return false;
  }
  return parsed.toLocaleDateString("da-DK") === new Date().toLocaleDateString("da-DK");
}

function actionLabel(path: string): string {
  if (path.includes("/queue-process")) return "Queue processor";
  if (path.includes("/sync-saxo-sim-portfolio")) return "Saxo SIM portfolio sync";
  if (path.includes("/adopt-broker-portfolio")) return "Broker holdings adoption";
  if (path.includes("/sync-broker")) return "Broker sync";
  if (path.includes("/retry-failed")) return "Retry failed orders";
  if (path.includes("/reconcile-broker")) return "Saxo SIM portfolio sync";
  if (path.includes("/scheduler-cycle")) return "Scheduler cycle";
  if (path.includes("/decision-report")) return "Decision report";
  if (path.includes("/saxo/auth/start")) return "Saxo re-authentication";
  if (path.includes("/manage")) return "Order action";
  return "Action";
}

function summarizeActionResult(path: string, result: Record<string, unknown>): string {
  const label = actionLabel(path);
  if (path.includes("/retry-failed")) {
    const retried = Array.isArray(result.retried) ? result.retried.length : 0;
    const skipped = Array.isArray(result.skipped) ? result.skipped.length : 0;
    return `${label} completed. Requeued ${retried} order${retried === 1 ? "" : "s"}${skipped ? `, skipped ${skipped}` : ""}.`;
  }
  if (path.includes("/reconcile-broker")) {
    const created = Number(result.created ?? 0);
    const skipped = Array.isArray(result.skipped) ? result.skipped.length : 0;
    return `${label} completed. Created ${formatNumber(created, 0)} order${created === 1 ? "" : "s"}${skipped ? `, skipped ${skipped}` : ""}.`;
  }
  if (path.includes("/adopt-broker-portfolio")) {
    const adjustments = Array.isArray(result.adjustments) ? result.adjustments.length : 0;
    return `${label} completed. Applied ${adjustments} local ledger adjustment${adjustments === 1 ? "" : "s"}.`;
  }
  if (path.includes("/sync-saxo-sim-portfolio")) {
    const created = Number(result.created ?? 0);
    const skipped = Array.isArray(result.skipped) ? result.skipped.length : 0;
    return `${label} completed. Created ${formatNumber(created, 0)} order${created === 1 ? "" : "s"}${skipped ? `, skipped ${skipped}` : ""}.`;
  }
  if (path.includes("/sync-broker")) {
    const updated = Number(result.updated ?? 0);
    return `${label} completed. Updated ${formatNumber(updated, 0)} order${updated === 1 ? "" : "s"}.`;
  }
  if (path.includes("/queue-process")) {
    const queued = Number(result.orders_processed ?? result.processed ?? 0);
    return `${label} completed. Processed ${formatNumber(queued, 0)} order${queued === 1 ? "" : "s"}.`;
  }
  if (path.includes("/scheduler-cycle")) {
    const generated = Boolean(result.generated_decision);
    return `${label} completed${generated ? " and generated a decision report" : ""}.`;
  }
  if (path.includes("/decision-report")) {
    const status = String((result.report as Record<string, unknown> | undefined)?.status ?? result.status ?? "ok");
    if (status === "xai_fallback") {
      return `${label} completed with deterministic fallback because xAI timed out.`;
    }
    return `${label} completed with status ${status}.`;
  }
  if (path.includes("/saxo/auth/start")) {
    if (typeof result.authorize_url === "string") {
      return "Redirecting to Saxo authorization.";
    }
    return String(result.message ?? result.command ?? "Run the Saxo OAuth helper locally to renew the session.");
  }
  if (path.includes("/manage")) {
    const status = String(result.status ?? "ok");
    return `${label} completed with status ${status}.`;
  }
  return `${label} completed.`;
}

function WatchlistCategoryPanel({ category, nowMs }: { category: WatchlistCategory; nowMs: number }) {
  const rows = category.items ?? [];
  const quotedRows = rows.filter((row) => row.change_pct !== null && row.change_pct !== undefined);
  const leader = quotedRows[0] ?? null;
  const laggard = quotedRows.at(-1) ?? null;
  const coveragePct = category.target_limit > 0 ? rows.length / category.target_limit : 0;

  return (
    <section className="watchlist-category">
      <div className="watchlist-category-header">
        <div>
          <h3>{category.label}</h3>
          <p>
            Showing {formatNumber(rows.length, 0)} of target {formatNumber(category.target_limit, 0)}
            {" · "}
            universe {formatNumber(category.total_universe, 0)}
          </p>
        </div>
        <span className="status-chip neutral">{formatPercent(coveragePct)} target coverage</span>
      </div>
      <div className="mini-grid">
        <article className="mini-card">
          <div className="label">Daily Leader</div>
          <div className={`value ${signedClass(leader?.change_pct)}`}>{leader ? String(leader.symbol ?? "") : "n/a"}</div>
          <div className="subvalue">{leader ? `${formatPercent(leader.change_pct)} · ${formatLocalMoney(leader.current_price, leader.currency)}` : "No quote yet"}</div>
        </article>
        <article className="mini-card">
          <div className="label">Daily Laggard</div>
          <div className={`value ${signedClass(laggard?.change_pct)}`}>{laggard ? String(laggard.symbol ?? "") : "n/a"}</div>
          <div className="subvalue">{laggard ? `${formatPercent(laggard.change_pct)} · ${formatLocalMoney(laggard.current_price, laggard.currency)}` : "No quote yet"}</div>
        </article>
        <article className="mini-card">
          <div className="label">Quoted Names</div>
          <div className="value">{formatNumber(quotedRows.length, 0)}</div>
          <div className="subvalue">{formatNumber(rows.length - quotedRows.length, 0)} missing quotes</div>
        </article>
      </div>
      <div className="table-wrap watchlist-table">
        <table>
          <thead>
            <tr>
              <th>Symbol</th>
              <th>Name</th>
              <th>Decision</th>
              <th>Exchange</th>
              <th>Currency</th>
              <th>Price</th>
              <th>Daily Change</th>
              <th>Quote Status</th>
            </tr>
          </thead>
          <tbody>
            {rows.length > 0 ? (
              rows.map((row) => (
                <tr key={String(row.symbol ?? "")}>
                  <td>
                    <a href={toYahooFinanceUrl(String(row.symbol ?? ""))} target="_blank" rel="noreferrer">
                      {String(row.symbol ?? "")}
                    </a>
                  </td>
                  <td className="wrap-cell">{String(row.name ?? "")}</td>
                  <td>
                    <DecisionCell decision={row.decision} nowMs={nowMs} />
                  </td>
                  <td>{String(row.exchange ?? "")}</td>
                  <td>{String(row.currency ?? "")}</td>
                  <td>{row.current_price === null || row.current_price === undefined ? "n/a" : formatLocalMoney(row.current_price, row.currency)}</td>
                  <td className={signedClass(row.change_pct)}>{row.change_pct === null || row.change_pct === undefined ? "n/a" : formatPercent(row.change_pct)}</td>
                  <td className="wrap-cell">{String(row.quote_status ?? row.quote_source ?? "")}</td>
                </tr>
              ))
            ) : (
              <tr>
                <td colSpan={8}>
                  <div className="empty-state">
                    <strong>No watchlist rows available.</strong>
                    <span>Quotes may still be loading, or all symbols in this category are excluded by risk settings.</span>
                  </div>
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </section>
  );
}

function PromptPanel({ prompt }: { prompt: Record<string, any> }) {
  const schema = prompt.schema ?? null;
  return (
    <article className="prompt-card">
      <div className="panel-header prompt-header">
        <div>
          <h3>{String(prompt.title ?? prompt.kind ?? "AI Prompt")}</h3>
          <p>{String(prompt.description ?? "Runtime prompt preview.")}</p>
        </div>
        <span className={`status-chip ${String(prompt.status ?? "ok") === "error" ? "bad" : "neutral"}`}>
          {String(prompt.status ?? "ok")}
        </span>
      </div>
      {prompt.error ? <div className="warning-box">{String(prompt.error)}</div> : null}
      {prompt.instruction ? (
        <section className="prompt-section">
          <div className="label">Core Instruction</div>
          <p>{String(prompt.instruction)}</p>
        </section>
      ) : null}
      <div className="prompt-grid">
        {prompt.system_prompt ? (
          <details className="json-details" open>
            <summary>System Prompt</summary>
            <pre className="code-block prompt-block">{String(prompt.system_prompt)}</pre>
          </details>
        ) : null}
        {prompt.user_prompt ? (
          <details className="json-details">
            <summary>User Prompt / Payload</summary>
            <pre className="code-block prompt-block">{String(prompt.user_prompt)}</pre>
          </details>
        ) : null}
      </div>
      {schema ? (
        <details className="json-details">
          <summary>Structured Output Schema</summary>
          <pre className="code-block prompt-block">{JSON.stringify(schema, null, 2)}</pre>
        </details>
      ) : null}
    </article>
  );
}

export function DashboardShell() {
  const [activeTab, setActiveTab] = useState<TabKey>("portfolio");
  const [performanceRange, setPerformanceRange] = useState<(typeof PERFORMANCE_RANGES)[number]>("1D");
  const [selectedDecisionId, setSelectedDecisionId] = useState<number | null>(null);
  const [selectedJournalId, setSelectedJournalId] = useState<number | null>(null);
  const [selectedSymbol, setSelectedSymbol] = useState<string | null>(null);
  const [statusMessage, setStatusMessage] = useState<string>("");
  const [statusDetails, setStatusDetails] = useState<string>("");
  const [statusTone, setStatusTone] = useState<"info" | "warn" | "good">("info");
  const [lastUpdatedAt, setLastUpdatedAt] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<string | null>(null);
  const [nowMs, setNowMs] = useState(() => Date.now());
  const [portfolioSort, setPortfolioSort] = useState<"unrealised" | "allocation">("allocation");
  const [cashModalOpen, setCashModalOpen] = useState<"add" | "reduce" | null>(null);
  const [reconciliationAction, setReconciliationAction] = useState<ReconciliationAction | null>(null);
  const [cashAdjustmentPct, setCashAdjustmentPct] = useState(5);
  const [cashBufferTargetPct, setCashBufferTargetPct] = useState(25);

  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    const stored = window.localStorage.getItem("daytrader-active-tab") as TabKey | null;
    if (stored && TAB_OPTIONS.some((tab) => tab.key === stored)) {
      setActiveTab(stored);
    }
  }, []);

  useEffect(() => {
    window.localStorage.setItem("daytrader-active-tab", activeTab);
  }, [activeTab]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const tagName = target?.tagName ?? "";
      if (target?.isContentEditable || tagName === "INPUT" || tagName === "TEXTAREA") {
        return;
      }
      if (event.key.toLowerCase() === "r") {
        event.preventDefault();
        void runAction("/api/actions/scheduler-cycle", { mock: false });
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  const overview = useSWR<OverviewResponse>("/api/overview", getFetcher, {
    refreshInterval: 15_000,
  });

  useEffect(() => {
    if (overview.data) {
      setLastUpdatedAt(new Date().toISOString());
    }
  }, [overview.data]);

  const priceRefreshMs = Math.max(
    15_000,
    Number(overview.data?.refresh?.price_poll_interval_minutes ?? 1) * 60_000,
  );

  const positions = useSWR<PositionsResponse>(
    activeTab === "portfolio" ? "/api/portfolio/positions?limit=25" : null,
    getFetcher,
    { refreshInterval: priceRefreshMs },
  );
  const performance = useSWR<PerformanceResponse>(
    activeTab === "performance" ? `/api/performance?range_key=${performanceRange}` : null,
    getFetcher,
    { refreshInterval: priceRefreshMs },
  );
  const market = useSWR<MarketResponse>(
    activeTab === "market" ? "/api/market/status" : null,
    getFetcher,
    { refreshInterval: 60_000 },
  );
  const watchlists = useSWR<WatchlistsResponse>(
    activeTab === "watchlist" ? "/api/market/watchlists" : null,
    getFetcher,
    { refreshInterval: priceRefreshMs },
  );
  const decision = useSWR<DecisionResponse>(
    activeTab === "decision" ? "/api/decision/latest" : null,
    getFetcher,
    { refreshInterval: 60_000 },
  );
  const decisionHistory = useSWR<DecisionHistoryResponse>(
    activeTab === "decision" ? "/api/decision/reports?limit=20" : null,
    getFetcher,
    { refreshInterval: 60_000 },
  );
  const strategyJournal = useSWR<StrategyJournalResponse>(
    activeTab === "decision" || activeTab === "journal" ? "/api/strategy-journal?limit=20" : null,
    getFetcher,
    { refreshInterval: 300_000 },
  );
  const prompts = useSWR<PromptsResponse>(
    activeTab === "prompts" ? "/api/prompts" : null,
    getFetcher,
    { refreshInterval: 300_000 },
  );
  const execution = useSWR<ExecutionResponse>(
    activeTab === "execution" ? "/api/execution?limit=150" : null,
    getFetcher,
    { refreshInterval: 15_000 },
  );
  const scheduler = useSWR<SchedulerResponse>(
    activeTab === "execution" ? "/api/scheduler?limit=10" : null,
    getFetcher,
    { refreshInterval: 30_000 },
  );
  const authSession = useSWR<AuthSession>("/auth/session", getFetcher, {
    refreshInterval: 300_000,
  });
  const saxoAuth = useSWR<SaxoAuthStatus>("/api/saxo/auth/status", getFetcher, {
    refreshInterval: 10_000,
    fallbackData: overview.data?.saxo_auth,
  });

  async function runAction(path: string, body?: unknown) {
    setPendingAction(path);
    let actionResponded = false;
    try {
      const result = await postAction<Record<string, unknown>>(path, body);
      actionResponded = true;
      if (path.includes("/saxo/auth/start") && typeof result.authorize_url === "string") {
        setStatusTone("info");
        setStatusMessage(summarizeActionResult(path, result));
        setStatusDetails("");
        setPendingAction(null);
        window.location.assign(result.authorize_url);
        return;
      }
      setStatusTone(result.status === "manual_required" ? "warn" : "good");
      setStatusMessage(summarizeActionResult(path, result));
      setStatusDetails(JSON.stringify(result, null, 2));
      setPendingAction(null);
      void Promise.all([
        mutate("/api/overview"),
        mutate("/api/portfolio/positions?limit=25"),
        mutate(`/api/performance?range_key=${performanceRange}`),
        mutate("/api/market/watchlists"),
        mutate("/api/execution?limit=150"),
        mutate("/api/decision/latest"),
        mutate("/api/decision/reports?limit=20"),
        mutate("/api/prompts"),
        mutate("/api/scheduler?limit=10"),
        mutate("/api/saxo/auth/status"),
      ]).catch((error) => {
        console.warn("Background refresh after action failed", error);
      });
    } catch (error) {
      setStatusTone("warn");
      setStatusMessage(error instanceof Error ? error.message : "Action failed.");
      setStatusDetails("");
    } finally {
      if (!actionResponded) {
        setPendingAction(null);
      }
    }
  }

  async function saveCashBufferSettings() {
    const actionPath = "/api/settings/cash-buffer";
    setPendingAction(actionPath);
    try {
      const result = await postAction<Record<string, unknown>>(actionPath, {
        min_cash_buffer_pct: cashBufferTargetPct / 100,
      });
      setStatusTone("good");
      setStatusMessage(`Cash buffer target updated to ${formatCashBufferPct(cashBufferTargetPct)}.`);
      setStatusDetails(JSON.stringify(result, null, 2));
      setCashModalOpen(null);
      await Promise.all([
        mutate("/api/overview"),
        mutate("/api/decision/latest"),
        mutate("/api/decision/reports?limit=20"),
        mutate("/api/execution?limit=150"),
      ]);
    } catch (error) {
      setStatusTone("warn");
      setStatusMessage(error instanceof Error ? error.message : "Cash buffer update failed.");
      setStatusDetails("");
    } finally {
      setPendingAction(null);
    }
  }

  const summary = overview.data?.portfolio_summary ?? {};
  const afterTaxSummary = overview.data?.after_tax_summary ?? {};
  const cashBufferSettings = overview.data?.settings?.cash_buffer;
  const effectiveCashBufferPct = Number(cashBufferSettings?.min_cash_buffer_pct ?? 0.1) * 100;
  const integrityWarnings = overview.data?.integrity?.warnings ?? [];
  const analysisSummary = overview.data?.analysis_summary;
  const backendError = [
    overview.error,
    activeTab === "portfolio" ? positions.error : null,
    activeTab === "performance" ? performance.error : null,
    activeTab === "market" ? market.error : null,
    activeTab === "watchlist" ? watchlists.error : null,
    activeTab === "decision" ? decision.error || decisionHistory.error : null,
    activeTab === "prompts" ? prompts.error : null,
    activeTab === "journal" ? strategyJournal.error : null,
    activeTab === "execution" ? execution.error || scheduler.error : null,
  ].find(Boolean);
  const backendErrorMessage =
    backendError instanceof Error
      ? backendError.message
      : backendError
        ? "Backend data is temporarily unavailable."
        : "";

  const performanceSeries = useMemo(() => {
    return (performance.data?.history ?? []).map((row) => ({
      recordedAt: String(row.recorded_at ?? ""),
      portfolioValueDkk: Number(row.total_market_value_dkk ?? 0),
      cashDkk: Number(row.cash_balance_dkk ?? 0),
    }));
  }, [performance.data?.history]);

  const browserTimeZone = useMemo(() => Intl.DateTimeFormat().resolvedOptions().timeZone || "local time", []);

  const portfolioColumns = useMemo(
    () => [
      "Symbol",
      "Decision",
      "Ladder Status",
      "Trend",
      "Qty",
      "Currency",
      "Paid Price",
      "Current Price",
      "Cost Basis DKK",
      "Market Value DKK",
      "Unrealised P/L DKK",
      "FX Gain/Loss DKK",
      "Daily P/L DKK",
      "Allocation",
      "Quote Updated",
    ],
    [],
  );

  const latestDecision = decision.data?.report;
  const nextDecision = decision.data?.next_report ?? null;
  const decisionHistoryItems = decisionHistory.data?.items ?? [];
  const promptItems = prompts.data?.items ?? [];
  const strategyJournalItems = strategyJournal.data?.items ?? [];
  const dailyOrderCapacity = overview.data?.execution?.daily_order_capacity;
  const displayedDecision = useMemo(() => {
    if (selectedDecisionId !== null) {
      const selected = decisionHistoryItems.find((row) => Number(row.id) === selectedDecisionId);
      if (selected) {
        return selected;
      }
    }
    return latestDecision ?? decisionHistoryItems[0] ?? null;
  }, [decisionHistoryItems, latestDecision, selectedDecisionId]);
  const decisionSuggestions = Array.isArray(displayedDecision?.report_json?.suggested_trades)
    ? (displayedDecision?.report_json?.suggested_trades as Array<Record<string, unknown>>)
    : [];
  const selectedAssets = Array.isArray(displayedDecision?.report_json?.strategy_plan?.selected_assets)
    ? (displayedDecision?.report_json?.strategy_plan?.selected_assets as Array<Record<string, unknown>>)
    : [];
  const strategyPlan = (displayedDecision?.report_json?.strategy_plan ?? {}) as Record<string, unknown>;
  const strategyFlow = (strategyPlan.flow_counts ?? {}) as Record<string, unknown>;
  const swingOrders = Array.isArray(strategyPlan.swing_orders) ? (strategyPlan.swing_orders as Array<Record<string, unknown>>) : [];
  const ladderOrders = Array.isArray(strategyPlan.ladder_orders) ? (strategyPlan.ladder_orders as Array<Record<string, unknown>>) : [];
  const cashManagement = (displayedDecision?.report_json?.cash_management ?? {}) as Record<string, unknown>;
  const goalTracking = (performance.data?.goal_tracking ?? overview.data?.goal_tracking ?? {}) as Record<string, any>;
  const latestJournal = strategyJournalItems[0] ?? null;
  const latestJournalBenchmarks = (latestJournal?.metrics_json?.benchmark_indices?.regions ?? {}) as Record<string, any>;
  const displayedJournal = useMemo(() => {
    if (selectedJournalId !== null) {
      const selected = strategyJournalItems.find((row) => Number(row.id) === selectedJournalId);
      if (selected) return selected;
    }
    return latestJournal;
  }, [latestJournal, selectedJournalId, strategyJournalItems]);
  const displayedDiaryEnvelope = (displayedJournal?.diary_json ?? {}) as Record<string, any>;
  const displayedDiary = (displayedDiaryEnvelope.diary ?? displayedDiaryEnvelope ?? {}) as Record<string, any>;
  const displayedJournalMetrics = (displayedJournal?.metrics_json ?? {}) as Record<string, any>;
  const displayedJournalBenchmarks = (displayedJournalMetrics.benchmark_indices?.regions ?? {}) as Record<string, any>;
  const displayedJournalLearnings = stringList(displayedJournal?.learnings_json);
  const tradingManager = (overview.data?.trading_manager ?? {}) as Record<string, any>;
  const tradingManagerStatus = (tradingManager.status ?? {}) as Record<string, any>;
  const latestTradingManagerRun = (tradingManager.latest_run ?? null) as Record<string, any> | null;
  const isSaxoAdapter = String(overview.data?.execution?.adapter ?? "").toLowerCase() === "saxo";
  const saxoEnvironment = String(saxoAuth.data?.environment ?? overview.data?.saxo_auth?.environment ?? "").toLowerCase();
  const isSaxoSim = isSaxoAdapter && saxoEnvironment === "sim";
  const isSaxoBrokerMode = isSaxoAdapter && saxoEnvironment !== "" && saxoEnvironment !== "sim";
  const simReconciliationAction: ReconciliationAction = {
    path: "/api/actions/reconcile-broker",
    title: "Reconcile Portfolio To Saxo SIM",
    buttonLabel: "⇄ Reconcile Portfolio To Saxo SIM",
    confirmLabel: "Reconcile To Saxo SIM",
    direction: "Local imported portfolio → Saxo Developer SIM account",
    details: [
      "The imported/local portfolio remains the source of truth.",
      "The backend compares local target quantities with Saxo SIM holdings.",
      "It queues/submits SIM-only orders to buy missing shares or sell excess SIM-only shares.",
      "It is blocked by the backend unless the configured and active Saxo session environment is SIM.",
    ],
    warning: "This can create Saxo SIM orders, but it must not alter Saxo Live holdings.",
  };
  const brokerAdoptionAction: ReconciliationAction = {
    path: "/api/actions/adopt-broker-portfolio",
    title: "Adopt Broker Holdings Into Local Ledger",
    buttonLabel: "⇣ Adopt Broker Holdings Into Local Ledger",
    confirmLabel: "Adopt Broker Holdings",
    direction: "Saxo broker account → local ledger/tax lots",
    details: [
      "The Saxo broker account becomes the source of truth for held quantities.",
      "The backend refreshes Saxo broker position, balance, account, and exposure snapshots first.",
      "It writes reconciliation adjustments so local portfolio quantities match broker holdings.",
      "It is blocked by the backend while the active Saxo session is SIM.",
    ],
    warning: "This can replace the imported/local portfolio view with broker holdings. Use it only when the broker account is authoritative.",
  };
  const visibleReconciliationAction = isSaxoSim ? simReconciliationAction : isSaxoBrokerMode ? brokerAdoptionAction : null;
  const friendlyDecisionMessage = decisionFriendlyMessage(displayedDecision ?? null, saxoAuth.data);
  const sortedPositions = useMemo(() => {
    const rows = [...(positions.data?.items ?? [])];
    const key = portfolioSort === "unrealised" ? "unrealised_pnl_dkk" : "allocation_pct";
    return rows.sort((left, right) => Number(right[key] ?? 0) - Number(left[key] ?? 0));
  }, [portfolioSort, positions.data?.items]);
  const dailyOrderCapacityPct =
    dailyOrderCapacity && dailyOrderCapacity.max > 0
      ? Math.min(Math.max((dailyOrderCapacity.used / dailyOrderCapacity.max) * 100, 0), 100)
      : 0;
  const cashDeploymentPct =
    Number(summary.total_market_value_dkk ?? 0) > 0
      ? (Number(summary.invested_market_value_dkk ?? 0) / Number(summary.total_market_value_dkk ?? 1)) * 100
      : 0;
  const cashBufferDisabled = effectiveCashBufferPct <= 0;
  const cashBufferShortfall = Number(cashManagement.cash_buffer_shortfall_dkk ?? 0);

  function openCashBufferModal(mode: "add" | "reduce") {
    setCashBufferTargetPct(normalizeCashBufferPct(effectiveCashBufferPct));
    setCashModalOpen(mode);
  }

  useEffect(() => {
    if (!decisionHistoryItems.length && !latestDecision) {
      setSelectedDecisionId(null);
      return;
    }
    if (selectedDecisionId === null) {
      const fallbackId = Number(latestDecision?.id ?? decisionHistoryItems[0]?.id ?? 0);
      setSelectedDecisionId(fallbackId || null);
      return;
    }
    const existsInHistory = decisionHistoryItems.some((row) => Number(row.id) === selectedDecisionId);
    if (!existsInHistory && Number(latestDecision?.id) !== selectedDecisionId) {
      const fallbackId = Number(latestDecision?.id ?? decisionHistoryItems[0]?.id ?? 0);
      setSelectedDecisionId(fallbackId || null);
    }
  }, [decisionHistoryItems, latestDecision, selectedDecisionId]);

  useEffect(() => {
    if (!strategyJournalItems.length) {
      setSelectedJournalId(null);
      return;
    }
    if (selectedJournalId === null) {
      setSelectedJournalId(Number(strategyJournalItems[0]?.id ?? 0) || null);
      return;
    }
    const existsInHistory = strategyJournalItems.some((row) => Number(row.id) === selectedJournalId);
    if (!existsInHistory) {
      setSelectedJournalId(Number(strategyJournalItems[0]?.id ?? 0) || null);
    }
  }, [selectedJournalId, strategyJournalItems]);

  const executionOrders = execution.data?.orders ?? [];
  const executionFills = execution.data?.fills ?? [];
  const manageableOrders = executionOrders.filter((row) =>
    [
      "submitted_to_broker",
      "broker_working",
      "broker_amended",
      "broker_partially_filled",
      "broker_replace_requested",
      "broker_cancel_requested",
    ].includes(String(row.status ?? "")),
  );

  const ladderSummary = useMemo(() => {
    const ladderOrders = executionOrders.filter((row) => String(row.strategy_type ?? "") === "ladder");
    const activeLadders = new Set(
      ladderOrders
        .filter((row) =>
          [
            "pending_execution",
            "pending_approval",
            "waiting_for_market_open",
            "submitted_to_broker",
            "broker_working",
            "broker_amended",
            "broker_partially_filled",
            "broker_replace_requested",
            "broker_cancel_requested",
          ].includes(String(row.status ?? "")),
        )
        .map((row) => String(row.symbol ?? "")),
    ).size;
    const filledRungs = ladderOrders.filter((row) => String(row.strategy_role ?? "") === "entry" && String(row.status ?? "") === "executed").length;
    return { activeLadders, filledRungs };
  }, [executionOrders]);

  return (
    <main className="shell">
      <header className="page-header">
        <div className="title-block">
          <h1>{overview.data?.app?.project_name ?? "saxo-daytrader-xai"}</h1>
          <p>
            Modern web frontend over the existing Python trading runtime. Targeted polling keeps the active
            view fresh without re-running the whole page.
          </p>
          <p className="muted">Last updated {timeAgo(lastUpdatedAt, nowMs)} · Shortcut: R runs one scheduler cycle</p>
        </div>
        <div className="header-actions">
          <div className="pill-row">
            <span className="pill">Execution: {String(overview.data?.execution?.mode ?? "n/a").toUpperCase()}</span>
            <span className="pill">Adapter: {overview.data?.execution?.adapter ?? "n/a"}</span>
            <span className="pill">Environment: {overview.data?.app?.environment ?? "n/a"}</span>
            <SaxoStatusPill
              status={saxoAuth.data}
              pending={pendingAction === "/api/saxo/auth/start"}
              onReauth={() => runAction("/api/saxo/auth/start")}
            />
          </div>
          <div className="user-menu" title={authSession.data?.user?.email ?? "Local session"}>
            <div className="avatar" aria-hidden="true">
              {userInitials(authSession.data)}
            </div>
            <div className="user-copy">
              <span>{authSession.data?.user?.name ?? "Local user"}</span>
              <small>{authSession.data?.user?.email ?? "Not behind ngrok OAuth"}</small>
            </div>
            {authSession.data?.authenticated ? (
              <a className="logout-link" href="/ngrok/logout">
                Logout
              </a>
            ) : null}
          </div>
        </div>
      </header>

      {backendErrorMessage ? (
        <section className="banner warn">
          Backend data is temporarily unavailable. Showing the latest data the frontend still has; polling will retry automatically.
          <span className="banner-inline-detail">{backendErrorMessage}</span>
        </section>
      ) : null}

      {integrityWarnings.map((warning) => (
        <section className="banner warn" key={warning}>
          {warning}
        </section>
      ))}

      {!cashBufferDisabled && cashBufferShortfall > 0 ? (
        <section className="banner warn banner-with-actions">
          <span>Cash buffer is below target by {formatDkk(cashManagement.cash_buffer_shortfall_dkk)}. Add cash or reduce exposure.</span>
          <span className="banner-action-row">
            <button className="ghost-button small" type="button" onClick={() => openCashBufferModal("add")}>
              Add cash
            </button>
            <button className="ghost-button small" type="button" onClick={() => openCashBufferModal("reduce")}>
              Reduce exposure
            </button>
          </span>
        </section>
      ) : null}

      {analysisSummary?.analysis_window_active ? (
        <section className="banner good">
          Analysis window active for {analysisSummary.active_windows.join(", ")}.
        </section>
      ) : analysisSummary?.pre_sync_markets?.length ? (
        <section className="banner info">
          Pre-analysis broker sync window active for {analysisSummary.pre_sync_markets.join(", ")}.
        </section>
      ) : (
        <section className="banner info">Analysis window inactive right now.</section>
      )}

      <section className="metric-grid">
        <article className="metric-card">
          <div className="label">Portfolio Value</div>
          <div className="value">{formatDkk(summary.total_market_value_dkk)}</div>
          <div className="subvalue">Invested {formatDkk(summary.invested_market_value_dkk)}</div>
        </article>
        <article className="metric-card">
          <div className="label">Cash</div>
          <div className="value">{formatDkk(summary.cash_balance_dkk)}</div>
          <div className="subvalue">
            Initial {formatDkk(summary.initial_cash_dkk)} · Trades {formatDkk(summary.cash_from_trades_dkk)}
          </div>
          <button className="ghost-button small metric-inline-action" type="button" onClick={() => openCashBufferModal("add")}>
            Cash buffer {formatCashBufferPct(effectiveCashBufferPct)}
          </button>
        </article>
        <article className="metric-card">
          <div className="label">Unrealised P/L</div>
          <div className={`value ${signedClass(summary.total_unrealised_pnl_dkk)}`}>
            {formatDkk(summary.total_unrealised_pnl_dkk)}
          </div>
          <div className="subvalue">
            After tax {formatDkk(afterTaxSummary.after_tax_unrealised_pnl_dkk)}
          </div>
        </article>
        <article className="metric-card">
          <div className="label">Daily P/L Since 06:00</div>
          <div className={`value ${signedClass(summary.total_daily_pnl_dkk)}`}>
            {formatDkk(summary.total_daily_pnl_dkk)}
          </div>
          <div className="subvalue">
            Open {formatDkk(summary.total_open_daily_pnl_dkk ?? summary.total_daily_pnl_dkk)} · Realised{" "}
            {formatDkk(summary.total_realised_daily_pnl_dkk ?? 0)}
          </div>
          <div className="subvalue">{formatNumber(summary.position_count, 0)} positions</div>
        </article>
      </section>

      <nav className="tabs" aria-label="Primary dashboard tabs">
        {TAB_OPTIONS.map((tab) => (
          <button
            key={tab.key}
            className={`tab ${activeTab === tab.key ? "active" : ""}`}
            onClick={() => setActiveTab(tab.key)}
            type="button"
          >
            {tab.label}
          </button>
        ))}
      </nav>

      {statusMessage ? (
        <section className={`banner ${statusTone}`}>
          <div>{statusMessage}</div>
          {statusDetails ? (
            <details className="banner-details">
              <summary>Show action details</summary>
              <pre className="code-block compact">{statusDetails}</pre>
            </details>
          ) : null}
        </section>
      ) : null}

      {activeTab === "portfolio" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>Portfolio Snapshot</h2>
              <p>Broker-aligned live holdings with a capped local budget model for new buys.</p>
            </div>
            <div className="sort-controls" aria-label="Portfolio sorting">
              <span className="muted">Sort by</span>
              <button className={`range-button ${portfolioSort === "allocation" ? "active" : ""}`} type="button" onClick={() => setPortfolioSort("allocation")}>
                Allocation
              </button>
              <button className={`range-button ${portfolioSort === "unrealised" ? "active" : ""}`} type="button" onClick={() => setPortfolioSort("unrealised")}>
                Unrealised P/L
              </button>
            </div>
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  {portfolioColumns.map((column) => (
                    <th key={column}>
                      <span className="help-header" title={PORTFOLIO_COLUMN_HELP[column]}>
                        {column}
                      </span>
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {sortedPositions.length > 0 ? (
                  sortedPositions.map((row) => (
                    <PortfolioRow key={String(row.symbol ?? "")} row={row} onOpen={setSelectedSymbol} nowMs={nowMs} />
                  ))
                ) : (
                  <tr>
                    <td colSpan={portfolioColumns.length}>
                      <div className="empty-state">
                        <strong>No open broker positions right now.</strong>
                        <span>
                          The portfolio is currently cash-only. If this follows the session-close flatten window, the
                          executed sell orders are listed in the Execution tab.
                        </span>
                      </div>
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </section>
      ) : null}

      {activeTab === "performance" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>Performance</h2>
              <p>
                Portfolio value history and progress against the {formatDkk(performance.data?.goal_tracking?.weekly_target_dkk)} weekly /
                {formatDkk(performance.data?.goal_tracking?.monthly_target_dkk)} monthly before-tax goals.
              </p>
            </div>
            <div className="range-picker">
              {PERFORMANCE_RANGES.map((range) => (
                <button
                  key={range}
                  className={`range-button ${performanceRange === range ? "active" : ""}`}
                  type="button"
                  onClick={() => setPerformanceRange(range)}
                >
                  {range}
                </button>
              ))}
            </div>
          </div>
          <div className="legend-row compact">
            <span className="legend-item">
              <span className="legend-dot" style={{ background: "#0f8a4b" }} />
              Portfolio value
            </span>
            <span className="legend-item">
              <span className="legend-dot" style={{ background: "#2563eb" }} />
              Cash balance
            </span>
          </div>
          <LineChart
            points={performanceSeries}
            positive={(performanceSeries.at(-1)?.portfolioValueDkk ?? 0) >= (performanceSeries[0]?.portfolioValueDkk ?? 0)}
          />
          <div className="mini-grid">
            {["day", "week", "month", "year", "all_time"].map((periodKey) => {
              const period = (performance.data?.goal_tracking?.periods?.[periodKey] ?? {}) as Record<string, unknown>;
              return (
                <article className="mini-card" key={periodKey}>
                  <div className="label">{periodKey.replace("_", " ").toUpperCase()}</div>
                  <div className={`value ${signedClass(period.pnl_dkk)}`}>{formatDkk(period.pnl_dkk)}</div>
                  <div className="muted">
                    Target {metricSubvalue(period.target_dkk, formatDkk)} · Gap {metricSubvalue(period.gap_dkk, formatDkk)}
                  </div>
                </article>
              );
            })}
          </div>
        </section>
      ) : null}

      {activeTab === "market" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>Market Status</h2>
              <p>Tradability, analysis windows, and calendar timing for tracked exchanges. Times below are shown in {browserTimeZone}.</p>
            </div>
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Exchange</th>
                  <th>Status</th>
                  <th>Tradable</th>
                  <th>Session Open ({browserTimeZone})</th>
                  <th>Tradable Close ({browserTimeZone})</th>
                  <th>Pre-Sync</th>
                  <th>Open Window</th>
                  <th>Close Window</th>
                  <th>Next Open ({browserTimeZone})</th>
                </tr>
              </thead>
              <tbody>
                {(market.data?.items ?? []).map((row) => (
                  <tr key={String(row.code)} className={isTodayTimestamp(row.session_open_at_utc) ? "today-row" : ""}>
                    <td>{String(row.market ?? row.code)}</td>
                    <td>{String(row.status_reason ?? "")}</td>
                    <td>{row.is_tradable ? "Yes" : "No"}</td>
                    <td>{row.session_open_at_utc ? formatTimestamp(row.session_open_at_utc) : "n/a"}</td>
                    <td>{row.tradable_close_at_utc ? formatTimestamp(row.tradable_close_at_utc) : "n/a"}</td>
                    <td>{row.pre_analysis_sync_active ? "Active" : "No"}</td>
                    <td>{row.open_analysis_window_active ? "Active" : "No"}</td>
                    <td>{row.close_analysis_window_active ? "Active" : "No"}</td>
                    <td>{row.next_open_at_utc ? formatTimestamp(row.next_open_at_utc) : "n/a"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </section>
      ) : null}

      {activeTab === "watchlist" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>Daily Watchlist Analysis</h2>
              <p>
                Quote-ranked stocks of interest for Nordic, UK, US, and EU/Euronext universes. Refreshed{" "}
                {formatTimestamp(watchlists.data?.generated_at)}.
              </p>
            </div>
            <div className="pill-row">
              {(watchlists.data?.categories ?? []).map((category) => (
                <span className="pill" key={category.key}>
                  {category.label}: {formatNumber(category.items.length, 0)} / {formatNumber(category.target_limit, 0)}
                </span>
              ))}
            </div>
          </div>
          <div className="watchlist-layout">
            {(watchlists.data?.categories ?? []).length > 0 ? (
              (watchlists.data?.categories ?? []).map((category) => (
                <WatchlistCategoryPanel category={category} key={category.key} nowMs={nowMs} />
              ))
            ) : (
              <div className="empty-state">
                <strong>Watchlist analysis is loading.</strong>
                <span>Quote collection can take a few seconds because the backend refreshes all category universes.</span>
              </div>
            )}
          </div>
        </section>
      ) : null}

      {activeTab === "decision" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>Decision Report</h2>
              <p>Latest xAI report plus deterministic strategy selection output.</p>
            </div>
            <div className="action-row">
              <ActionButton
                className="button"
                disabled={pendingAction !== null}
                loading={pendingAction === "/api/actions/decision-report"}
                onClick={() => runAction("/api/actions/decision-report")}
              >
                Generate Report
              </ActionButton>
            </div>
          </div>
          {friendlyDecisionMessage ? (
            <section className="friendly-status warn">
              <strong>{friendlyDecisionMessage}</strong>
              <span>Suggested next action: review the report details, cash buffer, and active market windows before manually forcing execution.</span>
            </section>
          ) : null}
          <div className="mini-grid">
            <article className="mini-card">
              <div className="label">Created</div>
              <div className="value">{formatTimestamp(displayedDecision?.created_at)}</div>
            </article>
            <article className="mini-card">
              <div className="label">Status</div>
              <div className="value">{decisionStatusLabel(displayedDecision)}</div>
            </article>
            <article className="mini-card">
              <div className="label">Selected Assets</div>
              <div className="value">{formatNumber(selectedAssets.length, 0)}</div>
            </article>
            <article className="mini-card">
              <div className="label">Suggested Trades</div>
              <div className="value">{formatNumber(decisionSuggestions.length, 0)}</div>
            </article>
            <article className="mini-card">
              <div className="label">Report Cadence</div>
              <div className="value">{overview.data?.refresh?.decision_cadence_label ?? "2 daily reports"}</div>
              <div className="subvalue">Nordic/EU +1h15 and US +1h15</div>
            </article>
            <article className="mini-card">
              <div className="label">Trading Manager</div>
              <div className={`value ${tradingManagerStatus.due ? "positive" : "neutral"}`}>
                {tradingManagerStatus.due ? "Due now" : latestTradingManagerRun?.status ?? "Waiting"}
              </div>
              <div className="subvalue">
                Next {String(tradingManagerStatus.next_pulse_label ?? "manager pulse")} · {formatTimestamp(tradingManagerStatus.next_pulse_at)}
              </div>
            </article>
            <article className="mini-card">
              <div className="label">Cash Buffer</div>
              <div className={`value ${cashBufferDisabled ? "neutral" : cashManagement.requires_cash_raise ? "negative" : "positive"}`}>
                {cashBufferDisabled ? "Disabled" : cashManagement.requires_cash_raise ? "Below target" : "Healthy"}
              </div>
              <div className="subvalue">
                Cash {formatDkk(cashManagement.cash_balance_dkk)} · Shortfall {formatDkk(cashBufferShortfall)}
              </div>
            </article>
            <article className="mini-card">
              <div className="label">Next Planned Report</div>
              <div className="value">{formatTimestamp(nextDecision?.next_report_at)}</div>
              <div className="subvalue">{String(nextDecision?.reason ?? "n/a")}</div>
            </article>
          </div>
          <div className="mini-grid">
            <article className="mini-card">
              <div className="label">Weekly Goal</div>
              <div className={`value ${signedClass(goalTracking?.periods?.week?.gap_dkk)}`}>
                {formatDkk(goalTracking?.periods?.week?.pnl_dkk)}
              </div>
              <div className="subvalue">
                Target-to-date {formatDkk(goalTracking?.periods?.week?.target_dkk)} · Full week{" "}
                {formatDkk(goalTracking?.periods?.week?.full_period_target_dkk ?? goalTracking?.weekly_target_dkk)}
              </div>
            </article>
            <article className="mini-card">
              <div className="label">Monthly Goal</div>
              <div className={`value ${signedClass(goalTracking?.periods?.month?.gap_dkk)}`}>
                {formatDkk(goalTracking?.periods?.month?.pnl_dkk)}
              </div>
              <div className="subvalue">
                Target-to-date {formatDkk(goalTracking?.periods?.month?.target_dkk)} · Full month{" "}
                {formatDkk(goalTracking?.periods?.month?.full_period_target_dkk ?? goalTracking?.monthly_target_dkk)}
              </div>
            </article>
            <article className="mini-card">
              <div className="label">Latest EOD Journal</div>
              <div className="value">{latestJournal ? String(latestJournal.journal_date ?? "n/a") : "n/a"}</div>
              <div className="subvalue">{latestJournal ? String(latestJournal.summary ?? "n/a") : "No journal entry recorded yet."}</div>
            </article>
            <article className="mini-card">
              <div className="label">Benchmark Context</div>
              <div className="subvalue">
                {Object.entries(latestJournalBenchmarks).length
                  ? Object.entries(latestJournalBenchmarks)
                      .map(([region, payload]) => `${region} ${formatPercent(Number(payload?.average_change_pct ?? 0))}`)
                      .join(" · ")
                  : "Available after the next end-of-day journal."}
              </div>
            </article>
          </div>
          <div className="grid-2">
            <div className="stack">
              <div className="mini-card">
                <div className="label">Strategy Flow</div>
                <div className="value">
                  {formatNumber(strategyFlow.macro_inputs ?? 0, 0)} → {formatNumber(strategyFlow.sentiment_symbols ?? 0, 0)} → {formatNumber(strategyFlow.constraint_checked ?? selectedAssets.length, 0)} → {formatNumber(strategyFlow.trade_count ?? swingOrders.length + ladderOrders.length, 0)}
                </div>
                <div className="subvalue">macro → sentiment → constraints → trades</div>
              </div>
              <div className="mini-card">
                <div className="label">Strategy Status</div>
                <div className="value">{String(strategyPlan.status ?? "n/a")}</div>
                <div className="muted">
                  {Array.isArray(strategyPlan.notes) ? (strategyPlan.notes as string[]).join(" ") : "n/a"}
                </div>
              </div>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Symbol</th>
                      <th>Action</th>
                      <th>Priority</th>
                      <th>Confidence</th>
                      <th>Rationale</th>
                    </tr>
                  </thead>
                  <tbody>
                    {decisionSuggestions.map((row, index) => (
                      <tr key={`${String(row.symbol ?? "symbol")}-${index}`}>
                        <td>{String(row.symbol ?? "")}</td>
                        <td>{String(row.action ?? "")}</td>
                        <td>{String(row.priority ?? "")}</td>
                        <td>{formatNumber(row.confidence, 0)}</td>
                        <td>{String(row.rationale ?? "")}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Selected</th>
                      <th>Score</th>
                      <th>Notes</th>
                    </tr>
                  </thead>
                  <tbody>
                    {selectedAssets.map((row, index) => (
                      <tr key={`${String(row.symbol ?? "asset")}-${index}`}>
                        <td>{String(row.symbol ?? "")}</td>
                        <td>{formatNumber(row.score, 2)}</td>
                        <td>{Array.isArray(row.notes) ? (row.notes as string[]).join(", ") : ""}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
              <div className="table-wrap">
                <table>
                  <thead>
                    <tr>
                      <th>Created</th>
                      <th>Status</th>
                      <th>Strategy</th>
                      <th>Selected</th>
                      <th>Trades</th>
                    </tr>
                  </thead>
                  <tbody>
                    {decisionHistoryItems.map((row) => {
                      const reportJson = (row.report_json ?? {}) as Record<string, unknown>;
                      const historyStrategy = (reportJson.strategy_plan ?? {}) as Record<string, unknown>;
                      const historySelected = Array.isArray(historyStrategy.selected_assets)
                        ? historyStrategy.selected_assets.length
                        : 0;
                      const historyTrades = Array.isArray(reportJson.suggested_trades)
                        ? reportJson.suggested_trades.length
                        : 0;
                      const isActive = Number(row.id) === selectedDecisionId;
                      return (
                        <tr
                          key={String(row.id)}
                          className={`history-row ${isActive ? "active" : ""}`}
                          onClick={() => setSelectedDecisionId(Number(row.id))}
                        >
                          <td>{formatTimestamp(row.created_at)}</td>
                          <td>{decisionStatusLabel(row)}</td>
                          <td>{String(historyStrategy.status ?? "n/a")}</td>
                          <td>{formatNumber(historySelected, 0)}</td>
                          <td>{formatNumber(historyTrades, 0)}</td>
                        </tr>
                      );
                    })}
                  </tbody>
                </table>
              </div>
            </div>
            <details className="json-details">
              <summary>Report JSON</summary>
              <pre className="code-block">{JSON.stringify(displayedDecision?.report_json ?? {}, null, 2)}</pre>
            </details>
          </div>
        </section>
      ) : null}

      {activeTab === "prompts" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>AI Prompts</h2>
              <p>
                Runtime prompt previews for the Decision Report, Trading Manager, and end-of-day diary. Generated{" "}
                {formatTimestamp(prompts.data?.generated_at)}.
              </p>
            </div>
            <div className="pill-row">
              <span className="pill">Decision #{String(prompts.data?.latest_decision_report?.id ?? "n/a")}</span>
              <span className="pill">Manager run #{String(prompts.data?.latest_trading_manager_run?.id ?? "n/a")}</span>
            </div>
          </div>
          <section className="friendly-status good">
            <strong>Trading Manager objective</strong>
            <span>
              Pick and manage stocks with conviction for daily, weekly, and monthly horizons. It should not flatten all
              positions just because the day is ending; selling requires thesis, technical, cash, risk, or opportunity evidence.
            </span>
          </section>
          {promptItems.length ? (
            promptItems.map((prompt) => <PromptPanel key={String(prompt.kind ?? prompt.title)} prompt={prompt} />)
          ) : (
            <div className="empty-state">
              <strong>Prompt previews are loading.</strong>
              <span>The backend builds these from the current runtime configuration and latest report context.</span>
            </div>
          )}
          {prompts.data?.latest_decision_report?.stored_prompt_text ? (
            <details className="json-details">
              <summary>Latest Stored Decision Prompt Text</summary>
              <pre className="code-block prompt-block">{String(prompts.data.latest_decision_report.stored_prompt_text)}</pre>
            </details>
          ) : null}
        </section>
      ) : null}

      {activeTab === "journal" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>End-of-Day Diary</h2>
              <p>xAI performance review, benchmark comparison, and lessons carried into future Decision Reports.</p>
            </div>
          </div>
          {displayedJournal ? (
            <>
              <div className="mini-grid">
                <article className="mini-card">
                  <div className="label">Journal Date</div>
                  <div className="value">{String(displayedJournal.journal_date ?? "n/a")}</div>
                  <div className="subvalue">{String(displayedJournal.cadence ?? "daily")} · {formatTimestamp(displayedJournal.created_at)}</div>
                </article>
                <article className="mini-card">
                  <div className="label">Diary Status</div>
                  <div className="value">{String(displayedJournalMetrics.diary_status ?? displayedDiaryEnvelope.status ?? "stored")}</div>
                  <div className="subvalue">Source report #{String(displayedJournal.source_report_id ?? "n/a")}</div>
                </article>
                <article className="mini-card">
                  <div className="label">Closed Trades</div>
                  <div className="value">{formatNumber(displayedJournalMetrics.trade_count ?? 0, 0)}</div>
                  <div className="subvalue">Realised {formatDkk(displayedJournalMetrics.realised_gain_dkk)}</div>
                </article>
                <article className="mini-card">
                  <div className="label">Week / Month</div>
                  <div className={`value ${signedClass(displayedJournalMetrics.goal_tracking?.periods?.week?.pnl_dkk)}`}>
                    {formatDkk(displayedJournalMetrics.goal_tracking?.periods?.week?.pnl_dkk)}
                  </div>
                  <div className="subvalue">Month {formatDkk(displayedJournalMetrics.goal_tracking?.periods?.month?.pnl_dkk)}</div>
                </article>
              </div>
              <div className="grid-2">
                <div className="stack">
                  <article className="mini-card">
                    <div className="label">Diary</div>
                    <div className="value">{String(displayedDiary.executive_summary ?? displayedJournal.summary ?? "n/a")}</div>
                  </article>
                  <div className="mini-grid">
                    <article className="mini-card">
                      <div className="label">What Went Well</div>
                      {stringList(displayedDiary.what_went_well).length ? (
                        stringList(displayedDiary.what_went_well).map((item) => <p className="muted" key={item}>{item}</p>)
                      ) : (
                        <p className="muted">No positives recorded.</p>
                      )}
                    </article>
                    <article className="mini-card">
                      <div className="label">What Did Not Work</div>
                      {stringList(displayedDiary.what_went_wrong).length ? (
                        stringList(displayedDiary.what_went_wrong).map((item) => <p className="muted" key={item}>{item}</p>)
                      ) : (
                        <p className="muted">No failures recorded.</p>
                      )}
                    </article>
                    <article className="mini-card">
                      <div className="label">Next Session Adjustments</div>
                      {stringList(displayedDiary.next_session_adjustments).length ? (
                        stringList(displayedDiary.next_session_adjustments).map((item) => <p className="muted" key={item}>{item}</p>)
                      ) : (
                        <p className="muted">No adjustments recorded.</p>
                      )}
                    </article>
                    <article className="mini-card">
                      <div className="label">Decision Report Memory</div>
                      {displayedJournalLearnings.length ? (
                        displayedJournalLearnings.map((item) => <p className="muted" key={item}>{item}</p>)
                      ) : (
                        <p className="muted">No learnings recorded.</p>
                      )}
                    </article>
                  </div>
                  <article className="mini-card">
                    <div className="label">Benchmark Readthrough</div>
                    <p>{String(displayedDiary.benchmark_readthrough ?? "n/a")}</p>
                  </article>
                  <div className="table-wrap">
                    <table>
                      <thead>
                        <tr>
                          <th>Region</th>
                          <th>Average</th>
                          <th>Indices</th>
                        </tr>
                      </thead>
                      <tbody>
                        {Object.entries(displayedJournalBenchmarks).map(([region, payload]) => (
                          <tr key={region}>
                            <td>{region}</td>
                            <td className={signedClass(payload?.average_change_pct)}>{formatPercent(payload?.average_change_pct)}</td>
                            <td className="wrap-cell">
                              {Array.isArray(payload?.items)
                                ? payload.items
                                    .map((item: Record<string, any>) => `${String(item.name ?? item.ticker)} ${formatPercent(item.change_pct)}`)
                                    .join(" · ")
                                : "n/a"}
                            </td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </div>
                <div className="stack">
                  <div className="table-wrap">
                    <table>
                      <thead>
                        <tr>
                          <th>Date</th>
                          <th>Cadence</th>
                          <th>Summary</th>
                        </tr>
                      </thead>
                      <tbody>
                        {strategyJournalItems.map((row) => {
                          const isActive = Number(row.id) === selectedJournalId;
                          return (
                            <tr
                              key={String(row.id)}
                              className={`history-row ${isActive ? "active" : ""}`}
                              onClick={() => setSelectedJournalId(Number(row.id))}
                            >
                              <td>{String(row.journal_date ?? "")}</td>
                              <td>{String(row.cadence ?? "")}</td>
                              <td className="wrap-cell">{String(row.summary ?? "")}</td>
                            </tr>
                          );
                        })}
                      </tbody>
                    </table>
                  </div>
                  <details className="json-details">
                    <summary>Diary JSON</summary>
                    <pre className="code-block">{JSON.stringify(displayedJournal ?? {}, null, 2)}</pre>
                  </details>
                </div>
              </div>
            </>
          ) : (
            <div className="empty-state">
              <strong>No end-of-day diary exists yet.</strong>
              <span>The scheduler creates one after the configured daily journal time.</span>
            </div>
          )}
        </section>
      ) : null}

      {activeTab === "execution" ? (
        <section className="panel stack">
          <div className="panel-header">
            <div>
              <h2>Execution</h2>
              <p>Queue control, broker sync, and live order management without page-wide reruns.</p>
              {dailyOrderCapacity ? (
                <p className="muted">
                  Daily executed-trade cap: {formatNumber(dailyOrderCapacity.used, 0)} / {formatNumber(dailyOrderCapacity.max, 0)} used
                  {" · "}
                  {formatNumber(dailyOrderCapacity.remaining, 0)} remaining
                </p>
              ) : null}
            </div>
            <div className="pill-row">
              <span className="pill">Queued {formatNumber(overview.data?.execution?.counts?.queued ?? 0, 0)}</span>
              <span className="pill">Broker Live {formatNumber(overview.data?.execution?.counts?.broker_live ?? 0, 0)}</span>
              <span className="pill">Failed {formatNumber(overview.data?.execution?.counts?.failed ?? 0, 0)}</span>
              <span className="pill">Fills {formatNumber(executionFills.length, 0)}</span>
            </div>
          </div>
          <section className={`broker-status-card ${saxoTone(saxoAuth.data)}`}>
            <div>
              <div className="label">Saxo Broker Status</div>
              <h3>
                <span className="status-dot" aria-hidden="true" />
                {saxoAuth.data?.connected ? "Connected" : saxoAuth.data?.needs_reauth ? "Re-authentication required" : "Token refresh available"}
              </h3>
              <p>{saxoAuth.data?.status_text ?? "Loading Saxo session status."}</p>
            </div>
            <div className="broker-status-grid">
              <span>
                Environment <strong>{String(saxoAuth.data?.environment ?? "n/a").toUpperCase()}</strong>
              </span>
              <span>
                Access token <strong>{saxoAuth.data?.token_valid ? "valid" : "not valid"}</strong>
              </span>
              <span>
                Expires <strong>{saxoAuth.data?.expires_in_minutes ?? "n/a"} min</strong>
              </span>
              <span>
                Refresh token <strong>{saxoAuth.data?.refresh_token_valid ? "valid" : "not valid"}</strong>
              </span>
              <span>
                Last refresh <strong>{formatTimestamp(saxoAuth.data?.last_refreshed_at)}</strong>
              </span>
            </div>
            <ActionButton
              className="ghost-button"
              disabled={pendingAction !== null}
              loading={pendingAction === "/api/saxo/auth/start"}
              onClick={() => runAction("/api/saxo/auth/start")}
            >
              Re-authenticate
            </ActionButton>
          </section>
          <div className="action-row">
            <ActionButton className="button" disabled={pendingAction !== null} loading={pendingAction === "/api/actions/queue-process"} onClick={() => runAction("/api/actions/queue-process")}>
              ▶ Run Queue Processor
            </ActionButton>
            <ActionButton className="ghost-button" disabled={pendingAction !== null} loading={pendingAction === "/api/actions/sync-broker"} onClick={() => runAction("/api/actions/sync-broker")}>
              ↻ Sync Broker Status
            </ActionButton>
            <ActionButton className="ghost-button" disabled={pendingAction !== null} loading={pendingAction === "/api/actions/retry-failed"} onClick={() => runAction("/api/actions/retry-failed")}>
              ↺ Retry Failed Orders
            </ActionButton>
            {visibleReconciliationAction ? (
              <ActionButton
                className="ghost-button"
                disabled={pendingAction !== null}
                loading={pendingAction === visibleReconciliationAction.path}
                onClick={() => setReconciliationAction(visibleReconciliationAction)}
              >
                {visibleReconciliationAction.buttonLabel}
              </ActionButton>
            ) : null}
            <ActionButton className="ghost-button" disabled={pendingAction !== null} loading={pendingAction === "/api/actions/scheduler-cycle"} onClick={() => runAction("/api/actions/scheduler-cycle", { mock: false })}>
              ⟳ Run Scheduler Cycle
            </ActionButton>
          </div>
          {!visibleReconciliationAction ? <p className="muted">Portfolio reconciliation actions are hidden until a Saxo session environment is available.</p> : null}
          {dailyOrderCapacity ? (
            <div className="cap-progress-block">
              <div className="muted">
                Daily executed-trade cap: {formatNumber(dailyOrderCapacity.used, 0)} / {formatNumber(dailyOrderCapacity.max, 0)}
              </div>
              <div className="cap-progress" aria-label="Daily executed trade cap progress">
                <div className="cap-progress-fill" style={{ width: `${dailyOrderCapacityPct}%` }} />
              </div>
            </div>
          ) : null}
          <div className="mini-grid">
            <article className="mini-card">
              <div className="label">Active Ladders</div>
              <div className="value">{formatNumber(ladderSummary.activeLadders, 0)}</div>
            </article>
            <article className="mini-card">
              <div className="label">Total Rungs Filled</div>
              <div className="value">{formatNumber(ladderSummary.filledRungs, 0)}</div>
            </article>
            <article className="mini-card">
              <div className="label">Executed Trades Left</div>
              <div className="value">{formatNumber(dailyOrderCapacity?.remaining ?? 0, 0)}</div>
              <div className="muted">Daily cap {formatNumber(dailyOrderCapacity?.max ?? 0, 0)}</div>
            </article>
          </div>
          <div className="panel-header compact-header">
            <div>
              <h3>Recent Broker Fills</h3>
              <p>Actual broker fill confirmations, separate from locally-created execution orders.</p>
            </div>
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>Fill Time</th>
                  <th>Order ID</th>
                  <th>Symbol</th>
                  <th>Side</th>
                  <th>Source</th>
                  <th>Status</th>
                  <th>Delta Qty</th>
                  <th>Cumulative Qty</th>
                  <th>Average Price</th>
                  <th>Ledger</th>
                </tr>
              </thead>
              <tbody>
                {executionFills.length ? (
                  executionFills.map((row) => (
                    <tr key={String(row.id)}>
                      <td>{formatTimestamp(row.created_at)}</td>
                      <td>{String(row.execution_order_id ?? "")}</td>
                      <td>{String(row.symbol ?? "")}</td>
                      <td>{String(row.side ?? "")}</td>
                      <td>{String(row.strategy_type ?? "manual")}</td>
                      <td>{String(row.fill_status ?? row.order_status ?? "")}</td>
                      <td>{formatNumber(row.delta_quantity, 0)}</td>
                      <td>{formatNumber(row.cumulative_quantity, 0)}</td>
                      <td>{formatLocalMoney(row.average_price_local, row.currency)}</td>
                      <td>{row.ledger_id ? String(row.ledger_id) : "broker-only"}</td>
                    </tr>
                  ))
                ) : (
                  <tr>
                    <td colSpan={10}>
                      <div className="empty-state">
                        <strong>No broker fills recorded yet.</strong>
                        <span>Executed orders will appear here once Saxo confirms a fill.</span>
                      </div>
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
          <div className="panel-header compact-header">
            <div>
              <h3>Execution Orders</h3>
              <p>Local order intents and their current broker state.</p>
            </div>
          </div>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th>ID</th>
                  <th>Created</th>
                  <th>Symbol</th>
                  <th>Action</th>
                  <th>Strategy</th>
                  <th>Role</th>
                  <th>Order Type</th>
                  <th>Status</th>
                  <th>Qty</th>
                  <th>Price</th>
                  <th>Limit</th>
                  <th>Stop</th>
                  <th>Error</th>
                  <th>Actions</th>
                </tr>
              </thead>
              <tbody>
                {executionOrders.map((row) => {
                  const isManageable = manageableOrders.some((order) => order.id === row.id);
                  return (
                    <tr key={String(row.id)} className={`status-row status-${String(row.status ?? "").replace(/[^a-z0-9]+/gi, "-").toLowerCase()}`}>
                      <td>{String(row.id)}</td>
                      <td>{formatTimestamp(row.created_at)}</td>
                      <td>{String(row.symbol ?? "")}</td>
                      <td>{String(row.action ?? "")}</td>
                      <td>{String(row.strategy_type ?? "manual")}</td>
                      <td>{String(row.strategy_role ?? "primary")}</td>
                      <td>{String(row.order_type ?? "Market")}</td>
                      <td>{String(row.status ?? "")}</td>
                      <td>{formatNumber(row.quantity, 0)}</td>
                      <td>{row.price_local ? formatLocalMoney(row.price_local, row.currency) : "n/a"}</td>
                      <td>{row.limit_price_local ? formatLocalMoney(row.limit_price_local, row.currency) : "n/a"}</td>
                      <td>{row.stop_price_local ? formatLocalMoney(row.stop_price_local, row.currency) : "n/a"}</td>
                      <td>{String(row.error_text ?? "")}</td>
                      <td>
                        {isManageable ? (
                          <div className="row-actions">
                            <button
                              className="danger-button"
                              type="button"
                              onClick={() => runAction(`/api/orders/${row.id}/manage`, { action: "cancel" })}
                            >
                              Cancel
                            </button>
                          </div>
                        ) : (
                          <span className="muted">n/a</span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <div className="grid-2">
            <div className="stack">
              <div className="mini-grid">
                {(scheduler.data?.cycles ?? []).slice(0, 4).map((cycle) => (
                  <article className="mini-card" key={String(cycle.id)}>
                    <div className="label cycle-label">
                      <span className={`cycle-dot ${String(cycle.status ?? "") === "ok" ? "good" : "bad"}`} aria-hidden="true" />
                      Cycle #{String(cycle.id)}
                    </div>
                    <div className="value">{String(cycle.status ?? "n/a")}</div>
                    <div className="muted">{formatTimestamp(cycle.started_at)}</div>
                  </article>
                ))}
              </div>
            </div>
            <pre className="code-block">{JSON.stringify(scheduler.data?.status ?? {}, null, 2)}</pre>
          </div>
        </section>
      ) : null}

      {cashModalOpen ? (
        <div className="modal-backdrop" role="presentation" onClick={() => setCashModalOpen(null)}>
          <section className="modal-card" role="dialog" aria-modal="true" aria-label="Cash buffer action" onClick={(event) => event.stopPropagation()}>
            <div className="panel-header">
              <div>
                <h2>Cash Buffer Settings</h2>
                <p>
                  Current deployment is {formatNumber(cashDeploymentPct, 1)}% with cash {formatDkk(summary.cash_balance_dkk)}.
                  The target controls how much portfolio value the strategy reserves as cash before placing new buys.
                </p>
              </div>
              <button className="ghost-button small" type="button" onClick={() => setCashModalOpen(null)}>
                Close
              </button>
            </div>
            <label className="slider-label">
              Cash buffer target: {formatCashBufferPct(cashBufferTargetPct)}
              <input
                type="range"
                min="0"
                max="50"
                step="0.1"
                value={cashBufferTargetPct}
                onChange={(event) => setCashBufferTargetPct(normalizeCashBufferPct(Number(event.target.value)))}
              />
            </label>
            <label className="slider-label">
              Planning shortcut: {formatNumber(cashAdjustmentPct, 0)}%
              <input
                type="range"
                min="1"
                max="25"
                value={cashAdjustmentPct}
                onChange={(event) => setCashAdjustmentPct(Number(event.target.value))}
              />
            </label>
            <div className="mini-grid">
              <article className="mini-card">
                <div className="label">{cashModalOpen === "add" ? "Estimated Cash To Add" : "Estimated Exposure To Reduce"}</div>
                <div className="value">{formatDkk((Number(summary.total_market_value_dkk ?? 0) * cashAdjustmentPct) / 100)}</div>
              </article>
              <article className="mini-card">
                <div className="label">New Strategy Guardrail</div>
                <div className="value">{cashBufferTargetPct <= 0 ? "Disabled" : `${formatCashBufferPct(cashBufferTargetPct)} cash`}</div>
                <div className="muted">
                  {cashBufferTargetPct <= 0
                    ? "The strategy may deploy all available cash subject to other constraints."
                    : `Deployment cap becomes ${formatNumber(100 - cashBufferTargetPct, cashBufferDecimals(cashBufferTargetPct))}%.`}
                </div>
              </article>
            </div>
            <div className="button-row">
              <ActionButton
                className="button"
                disabled={pendingAction !== null}
                loading={pendingAction === "/api/settings/cash-buffer"}
                onClick={() => void saveCashBufferSettings()}
              >
                Save Cash Buffer
              </ActionButton>
              <button className="ghost-button" type="button" onClick={() => setCashModalOpen(null)}>
                Cancel
              </button>
            </div>
          </section>
        </div>
      ) : null}

      {reconciliationAction ? (
        <div className="modal-backdrop" role="presentation" onClick={() => setReconciliationAction(null)}>
          <section className="modal-card" role="dialog" aria-modal="true" aria-label={reconciliationAction.title} onClick={(event) => event.stopPropagation()}>
            <div className="panel-header">
              <div>
                <h2>{reconciliationAction.title}</h2>
                <p>{reconciliationAction.direction}</p>
              </div>
              <button className="ghost-button small" type="button" onClick={() => setReconciliationAction(null)}>
                Close
              </button>
            </div>
            <div className="warning-box">
              <strong>Confirm direction before running.</strong>
              <span>{reconciliationAction.warning}</span>
            </div>
            <ul className="modal-list">
              {reconciliationAction.details.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
            <div className="mini-grid">
              <article className="mini-card">
                <div className="label">Current UI Portfolio</div>
                <div className="value">{formatNumber(summary.position_count ?? 0, 0)} positions</div>
                <div className="muted">Cash {formatDkk(summary.cash_balance_dkk)}</div>
              </article>
              <article className="mini-card">
                <div className="label">Saxo Session</div>
                <div className="value">{String(saxoEnvironment || "n/a").toUpperCase()}</div>
                <div className="muted">Adapter {String(overview.data?.execution?.adapter ?? "n/a")}</div>
              </article>
            </div>
            <div className="button-row">
              <ActionButton
                className={reconciliationAction.path.includes("adopt-broker") ? "danger-button" : "button"}
                disabled={pendingAction !== null}
                loading={pendingAction === reconciliationAction.path}
                onClick={async () => {
                  const action = reconciliationAction;
                  setReconciliationAction(null);
                  await runAction(action.path);
                }}
              >
                {reconciliationAction.confirmLabel}
              </ActionButton>
              <button className="ghost-button" type="button" onClick={() => setReconciliationAction(null)}>
                Cancel
              </button>
            </div>
          </section>
        </div>
      ) : null}

      <LadderVisualizer
        symbol={selectedSymbol ?? ""}
        open={selectedSymbol !== null}
        onClose={() => setSelectedSymbol(null)}
      />
    </main>
  );
}
