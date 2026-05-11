"use client";

import { useEffect, useMemo, useRef, useState } from "react";

import useSWR from "swr";
import {
  CandlestickSeries,
  ColorType,
  createChart,
  createSeriesMarkers,
  LineSeries,
  LineStyle,
  type SeriesMarker,
  type UTCTimestamp,
} from "lightweight-charts";

import { getFetcher } from "@/lib/api";
import { formatDkk, formatLocalMoney, formatNumber, formatPercent, formatTimestamp } from "@/lib/format";
import type { AssetLadderHistoryResponse } from "@/lib/types";

const RANGE_OPTIONS = ["1H", "4H", "SESSION"] as const;
const CHART_HEIGHT = 420;
const EMPTY_LIST: any[] = [];

interface LadderVisualizerProps {
  symbol: string;
  open: boolean;
  onClose: () => void;
}

function toChartTime(isoTime: string | null | undefined): UTCTimestamp | null {
  if (!isoTime) {
    return null;
  }
  const parsed = Date.parse(isoTime);
  if (!Number.isFinite(parsed)) {
    return null;
  }
  return Math.floor(parsed / 1000) as UTCTimestamp;
}

function markerColor(kind: string): string {
  if (kind === "buy_fill") return "#0f8a4b";
  if (kind === "sell_fill") return "#b42318";
  if (kind === "amendment") return "#38bdf8";
  if (kind === "flatten") return "#7c3aed";
  return "#2563eb";
}

function markerShape(kind: string): "circle" | "square" | "arrowUp" | "arrowDown" {
  if (kind === "buy_fill") return "arrowUp";
  if (kind === "sell_fill") return "arrowDown";
  if (kind === "amendment") return "square";
  return "circle";
}

function markerPosition(kind: string): "aboveBar" | "belowBar" | "inBar" {
  if (kind === "buy_fill") return "belowBar";
  if (kind === "sell_fill") return "aboveBar";
  return "inBar";
}

function markerYOffset(kind: string): number {
  if (kind === "buy_fill") return 18;
  if (kind === "sell_fill") return -18;
  if (kind === "amendment") return -10;
  return 0;
}

function withinRange(isoTime: string | null | undefined, startAtMs: number) {
  if (!isoTime) return false;
  const parsed = new Date(isoTime).getTime();
  return Number.isFinite(parsed) && parsed >= startAtMs;
}

export function LadderVisualizer({ symbol, open, onClose }: LadderVisualizerProps) {
  const [rangeKey, setRangeKey] = useState<(typeof RANGE_OPTIONS)[number]>("SESSION");
  const [showFills, setShowFills] = useState(true);
  const [showRungs, setShowRungs] = useState(true);
  const [showAmendments, setShowAmendments] = useState(true);
  const [selectedMarkerId, setSelectedMarkerId] = useState<string | null>(null);
  const [hoverMarker, setHoverMarker] = useState<{ marker: Record<string, any>; x: number; y: number } | null>(null);
  const [markerAnchors, setMarkerAnchors] = useState<Array<{ marker: Record<string, any>; x: number; y: number }>>([]);

  const chartContainerRef = useRef<HTMLDivElement | null>(null);
  const chartInstanceRef = useRef<any>(null);

  const history = useSWR<AssetLadderHistoryResponse>(
    open ? `/api/ladder-chart/${encodeURIComponent(symbol)}?range_key=${rangeKey}` : null,
    getFetcher,
    { refreshInterval: 30_000 },
  );

  const chartPoints = history.data?.chart?.points ?? EMPTY_LIST;
  const chartError = history.data?.chart?.error;
  const chartHasRealData = Boolean(history.data?.chart?.has_real_data);
  const chartSource = String(history.data?.chart?.source ?? "fallback");
  const markers = history.data?.markers ?? EMPTY_LIST;
  const activeLines = history.data?.active_lines ?? EMPTY_LIST;
  const ladderLevels = history.data?.ladder_levels ?? EMPTY_LIST;
  const ladderParameters = history.data?.ladder_parameters ?? {};
  const position = history.data?.position ?? null;
  const ladderSummary = history.data?.ladder_summary ?? {};
  const legend = history.data?.legend ?? [];

  const filteredMarkers = useMemo(() => {
    return markers.filter((marker) => {
      const kind = String(marker.kind ?? "");
      if (kind.includes("fill")) return showFills;
      if (kind === "amendment") return showAmendments;
      return true;
    });
  }, [markers, showAmendments, showFills]);

  const selectedMarker = filteredMarkers.find((marker) => String(marker.id) === selectedMarkerId) ?? filteredMarkers[0] ?? null;

  const visibleMarkers = useMemo(() => {
    const firstTime = toChartTime(String(chartPoints[0]?.time ?? "")) ?? null;
    const firstMs = firstTime ? Number(firstTime) * 1000 : 0;
    return filteredMarkers.filter((marker) => withinRange(String(marker.time ?? ""), firstMs));
  }, [chartPoints, filteredMarkers]);

  useEffect(() => {
    if (!open || !chartContainerRef.current) {
      return undefined;
    }

    const container = chartContainerRef.current;
    container.innerHTML = "";

    const chart = createChart(container, {
      width: container.clientWidth || 960,
      height: CHART_HEIGHT,
      layout: {
        background: { type: ColorType.Solid, color: "#ffffff" },
        textColor: "#657284",
        fontFamily: "\"Avenir Next\", \"Segoe UI\", \"Helvetica Neue\", sans-serif",
      },
      grid: {
        vertLines: { color: "rgba(215, 222, 232, 0.35)" },
        horzLines: { color: "rgba(215, 222, 232, 0.35)" },
      },
      rightPriceScale: {
        borderColor: "rgba(215, 222, 232, 0.8)",
      },
      timeScale: {
        borderColor: "rgba(215, 222, 232, 0.8)",
        timeVisible: true,
        secondsVisible: false,
        rightOffset: 8,
      },
      crosshair: {
        mode: 0,
      },
      handleScroll: true,
      handleScale: true,
    });
    chartInstanceRef.current = chart;

    const sortedPoints = [...chartPoints]
      .map((point) => {
        const time = toChartTime(String(point.time ?? ""));
        if (!time) {
          return null;
        }
        return {
          time,
          open: Number(point.open ?? point.close ?? 0),
          high: Number(point.high ?? point.close ?? 0),
          low: Number(point.low ?? point.close ?? 0),
          close: Number(point.close ?? point.open ?? 0),
        };
      })
      .filter(Boolean) as Array<{ time: UTCTimestamp; open: number; high: number; low: number; close: number }>;

    const isFallbackOnly = !chartHasRealData || chartSource !== "saxo";

    const baseSeries: any = isFallbackOnly
      ? chart.addSeries(LineSeries, {
          color: "#0f5c73",
          lineWidth: 2,
          priceLineVisible: false,
          lastValueVisible: false,
        })
      : chart.addSeries(CandlestickSeries, {
          upColor: "#0f8a4b",
          downColor: "#b42318",
          borderVisible: false,
          wickUpColor: "#0f8a4b",
          wickDownColor: "#b42318",
          priceLineVisible: false,
          lastValueVisible: false,
        });

    if (isFallbackOnly) {
      baseSeries.setData(
        sortedPoints.map((point) => ({ time: point.time, value: point.close })),
      );
    } else {
      baseSeries.setData(sortedPoints);
    }

    const overlaySeries: any[] = [];
    const xStart = sortedPoints[0]?.time ?? null;
    const xEnd = sortedPoints[sortedPoints.length - 1]?.time ?? null;
    const canDrawHorizontal = xStart !== null && xEnd !== null && xStart !== xEnd;

    if (canDrawHorizontal) {
      const lineRows: Array<Record<string, unknown> & { emphasized: boolean }> = [
        ...activeLines.map((row) => ({ ...row, emphasized: true })),
        ...(showRungs ? ladderLevels.map((row) => ({ ...row, emphasized: false })) : []),
      ];
      for (const line of lineRows) {
        const price = Number(line.price ?? 0);
        if (!Number.isFinite(price) || price <= 0) {
          continue;
        }
        const series = chart.addSeries(LineSeries, {
          color: String(line.color ?? "#9ca3af"),
          lineWidth: line.emphasized ? 2 : 1,
          lineStyle: line.emphasized ? LineStyle.LargeDashed : LineStyle.Dashed,
          priceLineVisible: false,
          lastValueVisible: false,
          crosshairMarkerVisible: false,
        });
        series.setData([
          { time: xStart, value: price },
          { time: xEnd, value: price },
        ]);
        overlaySeries.push(series);
      }

      if (position?.current_price_local) {
        const currentPrice = Number(position.current_price_local);
        if (Number.isFinite(currentPrice) && currentPrice > 0) {
          const currentSeries = chart.addSeries(LineSeries, {
            color: "#2563eb",
            lineWidth: 2,
            lineStyle: LineStyle.Solid,
            priceLineVisible: false,
            lastValueVisible: false,
            crosshairMarkerVisible: false,
          });
          currentSeries.setData([
            { time: xStart, value: currentPrice },
            { time: xEnd, value: currentPrice },
          ]);
          overlaySeries.push(currentSeries);
        }
      }
    }

    const seriesMarkers: SeriesMarker<UTCTimestamp>[] = visibleMarkers
      .map((marker) => {
        const time = toChartTime(String(marker.time ?? ""));
        if (!time) {
          return null;
        }
        const kind = String(marker.kind ?? "");
        return {
          id: String(marker.id),
          time,
          position: markerPosition(kind),
          shape: markerShape(kind),
          color: markerColor(kind),
          text: kind === "amendment" ? "A" : kind === "buy_fill" ? "B" : kind === "sell_fill" ? "S" : undefined,
          size: kind.includes("fill") ? 1.5 : 1,
        } satisfies SeriesMarker<UTCTimestamp>;
      })
      .filter(Boolean) as SeriesMarker<UTCTimestamp>[];
    createSeriesMarkers(baseSeries as any, seriesMarkers as any);

    const updateMarkerAnchors = () => {
      const anchors: Array<{ marker: Record<string, any>; x: number; y: number }> = [];
      for (const marker of visibleMarkers) {
        const markerTime = toChartTime(String(marker.time ?? ""));
        if (!markerTime) {
          continue;
        }
        const kind = String(marker.kind ?? "");
        const x = Number(chart.timeScale().timeToCoordinate(markerTime) ?? NaN);
        const baseY = Number(baseSeries.priceToCoordinate(Number(marker.price ?? 0)) ?? NaN);
        const y = baseY + markerYOffset(kind);
        if (!Number.isFinite(x) || !Number.isFinite(y)) {
          continue;
        }
        anchors.push({ marker, x, y });
      }
      setMarkerAnchors((current) => {
        if (
          current.length === anchors.length &&
          current.every((item, index) => {
            const next = anchors[index];
            return (
              item.marker?.id === next.marker?.id &&
              Math.abs(item.x - next.x) < 0.5 &&
              Math.abs(item.y - next.y) < 0.5
            );
          })
        ) {
          return current;
        }
        return anchors;
      });
    };

    chart.timeScale().fitContent();
    updateMarkerAnchors();

    chart.subscribeClick((param) => {
      if (!param.time || typeof param.time !== "number" || !visibleMarkers.length) {
        return;
      }
      const clickedAt = Number(param.time);
      const nearest = [...visibleMarkers].sort((left, right) => {
        const leftTime = Number(toChartTime(String(left.time ?? "")) ?? 0);
        const rightTime = Number(toChartTime(String(right.time ?? "")) ?? 0);
        return Math.abs(leftTime - clickedAt) - Math.abs(rightTime - clickedAt);
      })[0];
      if (nearest?.id) {
        setSelectedMarkerId(String(nearest.id));
      }
    });

    const resizeObserver = new ResizeObserver(() => {
      chart.resize(container.clientWidth || 960, CHART_HEIGHT);
      updateMarkerAnchors();
    });
    resizeObserver.observe(container);

    return () => {
      resizeObserver.disconnect();
      for (const series of overlaySeries) {
        chart.removeSeries(series);
      }
      chart.remove();
      chartInstanceRef.current = null;
    };
  }, [activeLines, chartHasRealData, chartPoints, chartSource, ladderLevels, open, position?.current_price_local, showRungs, visibleMarkers]);

  if (!open) {
    return null;
  }

  return (
    <div className="overlay" role="dialog" aria-modal="true" aria-label={`Ladder Visualizer ${symbol}`}>
      <div className="overlay-backdrop" onClick={onClose} />
      <div className="drawer">
        <header className="drawer-header">
          <div>
            <h2>Ladder Visualizer · {symbol}</h2>
            <p>
              {position ? String(position.instrument_name ?? symbol) : symbol} ·{" "}
              <span className={`status-chip ${ladderSummary.trailing ? "good" : "neutral"}`}>{String(ladderSummary.text ?? "idle")}</span>
            </p>
          </div>
          <button className="ghost-button" type="button" onClick={onClose}>
            Close
          </button>
        </header>

        <div className="mini-grid">
          <article className="mini-card">
            <div className="label">Paid Price</div>
            <div className="value">{position ? formatLocalMoney(position.paid_price_local, position.currency) : "n/a"}</div>
          </article>
          <article className="mini-card">
            <div className="label">Current Price</div>
            <div className="value">{position ? formatLocalMoney(position.current_price_local, position.currency) : "n/a"}</div>
          </article>
          <article className="mini-card">
            <div className="label">Unrealised P/L</div>
            <div className={`value ${Number(position?.unrealised_pnl_dkk ?? 0) >= 0 ? "positive" : "negative"}`}>
              {position ? formatDkk(position.unrealised_pnl_dkk) : "n/a"}
            </div>
          </article>
          <article className="mini-card">
            <div className="label">Cash Impact</div>
            <div className="value">{position ? formatDkk(position.market_value_dkk) : "n/a"}</div>
            <div className="subvalue">Allocation {position ? formatPercent(position.allocation_pct) : "0%"}</div>
          </article>
          <article className="mini-card">
            <div className="label">Ladder Parameters</div>
            <div className="value">
              {ladderParameters.rung_spacing_local !== null && ladderParameters.rung_spacing_local !== undefined
                ? formatLocalMoney(ladderParameters.rung_spacing_local, position?.currency)
                : "n/a"}
            </div>
            <div className="subvalue">
              ATR {ladderParameters.atr_1m ? formatNumber(ladderParameters.atr_1m, 3) : "n/a"} · Max {ladderParameters.max_position_weight_pct ? `${formatNumber(Number(ladderParameters.max_position_weight_pct), 1)}%` : "n/a"}
            </div>
          </article>
        </div>

        <div className="action-row ladder-controls">
          <div className="range-picker">
            {RANGE_OPTIONS.map((option) => (
              <button
                key={option}
                className={`range-button ${rangeKey === option ? "active" : ""}`}
                type="button"
                onClick={() => setRangeKey(option)}
              >
                {option}
              </button>
            ))}
            <button className="ghost-button" type="button" onClick={() => void history.mutate()}>
              Refresh Chart
            </button>
          </div>
          <label className="toggle"><input checked={showFills} onChange={(e) => setShowFills(e.target.checked)} type="checkbox" /> Show only fills</label>
          <label className="toggle"><input checked={showRungs} onChange={(e) => setShowRungs(e.target.checked)} type="checkbox" /> Show ladder rungs</label>
          <label className="toggle"><input checked={showAmendments} onChange={(e) => setShowAmendments(e.target.checked)} type="checkbox" /> Show amendments</label>
        </div>

        <div className="legend-row">
          {legend.map((item) => (
            <span className="legend-item" key={String(item.key)}>
              <span className="legend-dot" style={{ backgroundColor: String(item.color ?? "#9ca3af") }} />
              {String(item.label ?? item.key)}
            </span>
          ))}
        </div>

        <div className="grid-2 ladder-grid">
          <div className="chart-panel">
            {!chartHasRealData && chartError ? <div className="banner warn">{chartError}</div> : null}
            <div className="chart ladder-chart">
              <div className="chart-host" ref={chartContainerRef} />
              {markerAnchors.map(({ marker, x, y }) => (
                <button
                  key={`hitbox-${String(marker.id)}`}
                  type="button"
                  className="chart-marker-hitbox"
                  style={{ left: `${x}px`, top: `${y}px` }}
                  onMouseEnter={() => setHoverMarker({ marker, x, y })}
                  onMouseLeave={() => setHoverMarker((current) => (current?.marker?.id === marker.id ? null : current))}
                  onClick={() => setSelectedMarkerId(String(marker.id))}
                  aria-label={`Inspect ${String(marker.label ?? "event")}`}
                />
              ))}
              {hoverMarker ? (
                <div
                  className="chart-tooltip"
                  style={{
                    left: `${Math.min(hoverMarker.x + 14, (chartContainerRef.current?.clientWidth ?? 900) - 260)}px`,
                    top: `${Math.max(hoverMarker.y - 12, 12)}px`,
                  }}
                >
                  <div className="chart-tooltip-title">{String(hoverMarker.marker.label ?? "Event")}</div>
                  <div>{formatTimestamp(hoverMarker.marker.time)}</div>
                  <div>
                    {formatLocalMoney(hoverMarker.marker.price, position?.currency)} · Qty {formatNumber(hoverMarker.marker.quantity, 0)}
                  </div>
                  {hoverMarker.marker.strategy_reason ? <div>{String(hoverMarker.marker.strategy_reason)}</div> : null}
                </div>
              ) : null}
            </div>
          </div>
          <aside className="marker-panel">
            <div className="mini-card">
              <div className="label">Selected Event</div>
              {selectedMarker ? (
                <>
                  <div className="value">{String(selectedMarker.label ?? "Event")}</div>
                  <div className="muted">
                    {formatTimestamp(selectedMarker.time)} · {formatLocalMoney(selectedMarker.price, position?.currency)} · Qty {formatNumber(selectedMarker.quantity, 0)}
                  </div>
                  <div className="muted">{String(selectedMarker.details ?? "")}</div>
                  <div className="muted">
                    {selectedMarker.strategy_reason ? `Reason: ${String(selectedMarker.strategy_reason)}` : "Click a marker in the chart to inspect the event payload."}
                  </div>
                  <pre className="code-block compact">{JSON.stringify(selectedMarker.payload ?? {}, null, 2)}</pre>
                </>
              ) : (
                <div className="muted">Click any marker to inspect the order or fill payload.</div>
              )}
            </div>
          </aside>
        </div>
      </div>
    </div>
  );
}
