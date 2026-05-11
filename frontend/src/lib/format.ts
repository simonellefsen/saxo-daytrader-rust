export function formatDkk(value: unknown): string {
  const number = Number(value ?? 0);
  return new Intl.NumberFormat("da-DK", {
    style: "currency",
    currency: "DKK",
    maximumFractionDigits: 2,
  }).format(number);
}

export function formatLocalMoney(value: unknown, currency: unknown): string {
  const number = Number(value ?? 0);
  const safeCurrency = typeof currency === "string" && currency ? currency : "DKK";
  return new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: safeCurrency,
    maximumFractionDigits: 2,
  }).format(number);
}

export function formatNumber(value: unknown, digits = 2): string {
  return new Intl.NumberFormat("da-DK", {
    minimumFractionDigits: 0,
    maximumFractionDigits: digits,
  }).format(Number(value ?? 0));
}

export function formatPercent(value: unknown): string {
  return `${formatNumber(Number(value ?? 0) * 100, 2)}%`;
}

export function signedClass(value: unknown): string {
  const number = Number(value ?? 0);
  if (number > 0) {
    return "positive";
  }
  if (number < 0) {
    return "negative";
  }
  return "";
}

export function formatTimestamp(value: unknown): string {
  if (!value || typeof value !== "string") {
    return "n/a";
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return value;
  }
  return new Intl.DateTimeFormat("da-DK", {
    dateStyle: "short",
    timeStyle: "short",
  }).format(parsed);
}

export function formatTimestampPrecise(value: unknown): string {
  if (!value || typeof value !== "string") {
    return "n/a";
  }
  const parsed = new Date(value);
  if (Number.isNaN(parsed.getTime())) {
    return String(value);
  }
  return new Intl.DateTimeFormat("da-DK", {
    dateStyle: "short",
    timeStyle: "medium",
  }).format(parsed);
}

export function toYahooFinanceUrl(symbol: string): string {
  const [ticker, market] = symbol.split(":");
  const upperTicker = ticker.toUpperCase();
  const suffixMap: Record<string, string> = {
    xcse: ".CO",
    xsto: ".ST",
    xosl: ".OL",
    xhel: ".HE",
    xams: ".AS",
    xbru: ".BR",
    xlse: ".LS",
    xpar: ".PA",
    xmil: ".MI",
    xlon: ".L",
    xetr: ".DE",
    xnas: "",
    xnys: "",
  };
  return `https://finance.yahoo.com/quote/${upperTicker}${suffixMap[market] ?? ""}`;
}
