"use client";

interface SparklineProps {
  values: number[];
  positive: boolean;
}

export function Sparkline({ values, positive }: SparklineProps) {
  if (!values.length) {
    return <span className="muted">n/a</span>;
  }

  const width = 120;
  const height = 32;
  const min = Math.min(...values);
  const max = Math.max(...values);
  const spread = Math.max(max - min, 1e-9);
  const points = values.map((value, index) => {
    const x = (index / Math.max(values.length - 1, 1)) * width;
    const y = height - ((value - min) / spread) * height;
    return `${index === 0 ? "M" : "L"} ${x.toFixed(2)} ${y.toFixed(2)}`;
  });
  const stroke = positive ? "#0f8a4b" : "#b42318";

  return (
    <svg className="sparkline" viewBox={`0 0 ${width} ${height}`} aria-hidden="true">
      <path d={points.join(" ")} fill="none" stroke={stroke} strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
