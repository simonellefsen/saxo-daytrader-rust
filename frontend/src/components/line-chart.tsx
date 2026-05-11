"use client";

import { type PointerEvent, useState } from "react";

interface Point {
  x: number;
  y: number;
}

interface PerformancePoint {
  recordedAt: string;
  portfolioValueDkk: number;
  cashDkk: number;
}

interface LineChartProps {
  points: PerformancePoint[];
  positive: boolean;
}

function buildPath(points: Point[]): string {
  return points.map((point, index) => `${index === 0 ? "M" : "L"} ${point.x} ${point.y}`).join(" ");
}

function scaleValue(value: number, min: number, max: number, top: number, bottom: number): number {
  const spread = Math.max(max - min, 1);
  return bottom - ((value - min) / spread) * (bottom - top);
}

function formatAxisDkk(value: number): string {
  if (Math.abs(value) >= 1000) {
    return `${Math.round(value / 1000)}k`;
  }
  return `${Math.round(value)}`;
}

function formatTooltipDkk(value: number): string {
  return `${new Intl.NumberFormat("da-DK", {
    maximumFractionDigits: 2,
    minimumFractionDigits: 2,
  }).format(value)} kr.`;
}

function formatTimeLabel(timestamp: number, spanMs: number): string {
  const date = new Date(timestamp);
  const options: Intl.DateTimeFormatOptions =
    spanMs <= 48 * 60 * 60 * 1000
      ? { hour: "2-digit", minute: "2-digit" }
      : { day: "2-digit", month: "short" };
  return new Intl.DateTimeFormat("da-DK", options).format(date);
}

function linePoints(points: PerformancePoint[], valueKey: "portfolioValueDkk" | "cashDkk", min: number, max: number, top: number, bottom: number, left: number, right: number, startMs: number, spanMs: number): Point[] {
  return points.map((point, index) => {
    const timestamp = Date.parse(point.recordedAt);
    const x =
      spanMs > 0 && Number.isFinite(timestamp)
        ? left + ((timestamp - startMs) / spanMs) * (right - left)
        : left + (index / Math.max(points.length - 1, 1)) * (right - left);
    return {
      x,
      y: scaleValue(point[valueKey], min, max, top, bottom),
    };
  });
}

export function LineChart({ points, positive }: LineChartProps) {
  const [activeIndex, setActiveIndex] = useState<number | null>(null);
  const [activeCursorX, setActiveCursorX] = useState<number | null>(null);

  if (points.length === 0) {
    return <div className="chart muted">No portfolio history has been recorded yet.</div>;
  }

  const width = 920;
  const height = 300;
  const left = 72;
  const right = width - 72;
  const top = 20;
  const bottom = height - 42;
  const valueMin = Math.min(...points.map((point) => point.portfolioValueDkk));
  const valueMax = Math.max(...points.map((point) => point.portfolioValueDkk));
  const cashMin = Math.min(...points.map((point) => point.cashDkk));
  const cashMax = Math.max(...points.map((point) => point.cashDkk));
  const timestamps = points.map((point) => Date.parse(point.recordedAt)).filter(Number.isFinite);
  const startMs = timestamps.length > 0 ? Math.min(...timestamps) : 0;
  const endMs = timestamps.length > 0 ? Math.max(...timestamps) : 0;
  const spanMs = Math.max(endMs - startMs, 0);
  const portfolioPoints = linePoints(points, "portfolioValueDkk", valueMin, valueMax, top, bottom, left, right, startMs, spanMs);
  const cashPoints = linePoints(points, "cashDkk", cashMin, cashMax, top, bottom, left, right, startMs, spanMs);
  const displayPortfolioPoints = points.length === 1
    ? [
        { x: left, y: portfolioPoints[0].y },
        { x: right, y: portfolioPoints[0].y },
      ]
    : portfolioPoints;
  const displayCashPoints = points.length === 1
    ? [
        { x: left, y: cashPoints[0].y },
        { x: right, y: cashPoints[0].y },
      ]
    : cashPoints;
  const portfolioPath = buildPath(displayPortfolioPoints);
  const cashPath = buildPath(displayCashPoints);
  const areaPath = `${portfolioPath} L ${displayPortfolioPoints[displayPortfolioPoints.length - 1]?.x ?? right} ${bottom} L ${displayPortfolioPoints[0]?.x ?? left} ${bottom} Z`;
  const portfolioStroke = positive ? "#0f8a4b" : "#b42318";
  const cashStroke = "#2563eb";
  const yTicks = Array.from({ length: 4 }, (_, index) => index / 3);
  const xTickCount = Math.min(5, Math.max(points.length, 2));
  const xTicks = Array.from({ length: xTickCount }, (_, index) => {
    const ratio = xTickCount === 1 ? 0 : index / (xTickCount - 1);
    return startMs + ratio * spanMs;
  });
  const weekendBands = (() => {
    if (spanMs <= 0) {
      return [];
    }
    const bands: Array<{ x: number; width: number; key: string }> = [];
    const startDate = new Date(startMs);
    const cursor = new Date(startDate.getFullYear(), startDate.getMonth(), startDate.getDate());
    while (cursor.getTime() <= endMs) {
      const day = cursor.getDay();
      if (day === 0 || day === 6) {
        const dayStartMs = cursor.getTime();
        const nextDayMs = dayStartMs + 24 * 60 * 60 * 1000;
        const bandStartMs = Math.max(dayStartMs, startMs);
        const bandEndMs = Math.min(nextDayMs, endMs);
        if (bandEndMs > bandStartMs) {
          const x1 = left + ((bandStartMs - startMs) / spanMs) * (right - left);
          const x2 = left + ((bandEndMs - startMs) / spanMs) * (right - left);
          bands.push({ x: x1, width: x2 - x1, key: cursor.toISOString() });
        }
      }
      cursor.setDate(cursor.getDate() + 1);
    }
    return bands;
  })();
  const activePortfolioPoint = activeIndex === null ? null : portfolioPoints[activeIndex] ?? null;
  const activeCashPoint = activeIndex === null ? null : cashPoints[activeIndex] ?? null;
  const activeDataPoint = activeIndex === null ? null : points[activeIndex] ?? null;
  const activeTimestamp = activeDataPoint ? Date.parse(activeDataPoint.recordedAt) : null;
  const crosshairX = activeCursorX ?? activePortfolioPoint?.x ?? null;
  const tooltipX = activePortfolioPoint && crosshairX !== null
    ? crosshairX > left + (right - left) / 2
      ? Math.max(crosshairX - 224, left + 8)
      : Math.min(crosshairX + 14, right - 220)
    : left;
  const tooltipY = activePortfolioPoint
    ? Math.min(Math.max(activePortfolioPoint.y - 62, top + 8), bottom - 92)
    : top;

  function onPointerMove(event: PointerEvent<SVGSVGElement>) {
    const svg = event.currentTarget;
    const matrix = svg.getScreenCTM();
    if (!matrix) {
      return;
    }
    const point = svg.createSVGPoint();
    point.x = event.clientX;
    point.y = event.clientY;
    const svgPoint = point.matrixTransform(matrix.inverse());
    const viewX = Math.min(Math.max(svgPoint.x, left), right);
    if (viewX < left || viewX > right) {
      setActiveIndex(null);
      setActiveCursorX(null);
      return;
    }
    let nearestIndex = 0;
    let nearestDistance = Number.POSITIVE_INFINITY;
    portfolioPoints.forEach((point, index) => {
      const distance = Math.abs(point.x - viewX);
      if (distance < nearestDistance) {
        nearestDistance = distance;
        nearestIndex = index;
      }
    });
    setActiveIndex(nearestIndex);
    setActiveCursorX(viewX);
  }

  return (
    <div className="chart">
      <svg
        viewBox={`0 0 ${width} ${height}`}
        role="img"
        aria-label="Portfolio value chart"
        onPointerMove={onPointerMove}
        onPointerLeave={() => {
          setActiveIndex(null);
          setActiveCursorX(null);
        }}
      >
        <defs>
          <linearGradient id="portfolio-fill" x1="0" x2="0" y1="0" y2="1">
            <stop offset="0%" stopColor={portfolioStroke} stopOpacity="0.22" />
            <stop offset="100%" stopColor={portfolioStroke} stopOpacity="0.03" />
          </linearGradient>
        </defs>
        {weekendBands.map((band) => (
          <rect key={band.key} x={band.x} y={top} width={band.width} height={bottom - top} className="chart-weekend-band" />
        ))}
        {yTicks.map((ratio) => {
          const y = top + ratio * (bottom - top);
          const valueTick = valueMax - ratio * Math.max(valueMax - valueMin, 1);
          const cashTick = cashMax - ratio * Math.max(cashMax - cashMin, 1);
          return (
            <g key={`y-${ratio}`}>
              <line x1={left} x2={right} y1={y} y2={y} stroke="#d9e2ea" strokeWidth="1" />
              <text x={left - 10} y={y + 4} textAnchor="end" className="chart-axis chart-axis-value">
                {formatAxisDkk(valueTick)}
              </text>
              <text x={right + 10} y={y + 4} textAnchor="start" className="chart-axis chart-axis-cash">
                {formatAxisDkk(cashTick)}
              </text>
            </g>
          );
        })}
        {xTicks.map((timestamp) => {
          const x = spanMs > 0 ? left + ((timestamp - startMs) / spanMs) * (right - left) : left;
          return (
            <g key={`x-${timestamp}`}>
              <line x1={x} x2={x} y1={top} y2={bottom} stroke="#edf2f7" strokeWidth="1" />
              <text x={x} y={height - 12} textAnchor="middle" className="chart-axis">
                {formatTimeLabel(timestamp, spanMs)}
              </text>
            </g>
          );
        })}
        <path d={areaPath} fill="url(#portfolio-fill)" />
        <path d={portfolioPath} fill="none" stroke={portfolioStroke} strokeWidth="3" strokeLinejoin="round" strokeLinecap="round" />
        <path d={cashPath} fill="none" stroke={cashStroke} strokeWidth="2.5" strokeLinejoin="round" strokeLinecap="round" strokeDasharray="7 5" />
        {activePortfolioPoint && activeCashPoint && activeDataPoint && crosshairX !== null ? (
          <g className="chart-crosshair">
            <line x1={crosshairX} x2={crosshairX} y1={top} y2={bottom} />
            <line className="chart-crosshair-value" x1={left} x2={right} y1={activePortfolioPoint.y} y2={activePortfolioPoint.y} />
            <line className="chart-crosshair-cash" x1={left} x2={right} y1={activeCashPoint.y} y2={activeCashPoint.y} />
            <circle cx={activePortfolioPoint.x} cy={activePortfolioPoint.y} r="5" fill={portfolioStroke} />
            <circle cx={activeCashPoint.x} cy={activeCashPoint.y} r="4" fill={cashStroke} />
            <g>
              <rect x={left - 68} y={activePortfolioPoint.y - 13} width="58" height="24" rx="7" className="chart-axis-pill chart-axis-pill-value" />
              <text x={left - 39} y={activePortfolioPoint.y + 5} textAnchor="middle" className="chart-axis-pill-text">
                {formatAxisDkk(activeDataPoint.portfolioValueDkk)}
              </text>
              <rect x={right + 10} y={activeCashPoint.y - 13} width="58" height="24" rx="7" className="chart-axis-pill chart-axis-pill-cash" />
              <text x={right + 39} y={activeCashPoint.y + 5} textAnchor="middle" className="chart-axis-pill-text">
                {formatAxisDkk(activeDataPoint.cashDkk)}
              </text>
            </g>
            <g transform={`translate(${tooltipX} ${tooltipY})`}>
              <rect width="210" height="86" rx="14" className="chart-tooltip-box" />
              <text x="12" y="22" className="chart-tooltip-heading">
                {activeTimestamp ? formatTimeLabel(activeTimestamp, spanMs) : "Selected point"}
              </text>
              <text x="12" y="46" className="chart-tooltip-line chart-tooltip-value">
                Portfolio {formatTooltipDkk(activeDataPoint.portfolioValueDkk)}
              </text>
              <text x="12" y="68" className="chart-tooltip-line chart-tooltip-cash">
                Cash {formatTooltipDkk(activeDataPoint.cashDkk)}
              </text>
            </g>
          </g>
        ) : null}
        <g className="chart-legend">
          <circle cx={left} cy={top + 10} r="5" fill={portfolioStroke} />
          <text x={left + 10} y={top + 15}>Portfolio value</text>
          <circle cx={left + 140} cy={top + 10} r="5" fill={cashStroke} />
          <text x={left + 150} y={top + 15}>Cash</text>
        </g>
        <rect x={left} y={top} width={right - left} height={bottom - top} fill="transparent" />
      </svg>
    </div>
  );
}
