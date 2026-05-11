export type JsonObject = Record<string, any>;

export interface OverviewResponse {
  app: {
    project_name?: string;
    environment?: string;
    config_path?: string;
  };
  execution: {
    mode?: string;
    adapter?: string;
    require_approval_live: boolean;
    max_daily_orders: number;
    daily_order_capacity?: {
      max: number;
      used: number;
      remaining: number;
    };
    counts: Record<string, number>;
  };
  portfolio_summary: JsonObject;
  after_tax_summary: JsonObject;
  goal_tracking?: JsonObject;
  integrity: {
    healthy: boolean;
    warnings: string[];
    mismatches: Array<Record<string, any>>;
    unreconciled_orders: Array<Record<string, any>>;
  };
  analysis_summary: {
    analysis_window_active: boolean;
    active_markets: string[];
    active_windows: string[];
    pre_sync_markets: string[];
  };
  latest_decision: {
    id?: number | null;
    created_at?: string | null;
    status?: string | null;
  };
  scheduler_status: JsonObject | null;
  scheduler_health: JsonObject | null;
  trading_manager?: JsonObject | null;
  saxo_auth?: SaxoAuthStatus;
  settings?: {
    cash_buffer?: CashBufferSettings;
  };
  refresh: {
    price_poll_interval_minutes: number;
    scheduler_poll_interval_minutes: number;
    decision_cadence?: string;
    decision_cadence_label?: string;
    decision_pulses?: Array<Record<string, any>>;
    next_decision_pulse_at?: string | null;
    next_decision_pulse_label?: string | null;
  };
}

export interface CashBufferSettings {
  min_cash_buffer_pct: number;
  max_deployment_pct: number;
  source?: string;
  updated_at?: string | null;
  config_default_min_cash_buffer_pct?: number;
}

export interface SaxoAuthStatus {
  connected: boolean;
  environment: "sim" | "live" | string;
  configured_environment?: string;
  token_valid: boolean;
  refresh_token_valid?: boolean;
  expires_at: string | null;
  expires_in_minutes: number | null;
  refresh_expires_at?: string | null;
  refresh_expires_in_minutes?: number | null;
  last_refreshed_at?: string | null;
  refreshing: boolean;
  needs_reauth: boolean;
  status: string;
  status_text?: string;
  session_path?: string;
  error?: string | null;
}

export interface PositionsResponse {
  items: Array<Record<string, any>>;
  total: number;
}

export interface AssetLadderHistoryResponse {
  symbol: string;
  range_key: string;
  position: Record<string, any> | null;
  ladder_summary: Record<string, any>;
  ladder_parameters?: Record<string, any>;
  legend?: Array<Record<string, any>>;
  chart: {
    points: Array<Record<string, any>>;
    error?: string | null;
    source?: string | null;
    has_real_data?: boolean;
    first_event_at?: string | null;
  };
  markers: Array<Record<string, any>>;
  active_lines: Array<Record<string, any>>;
  ladder_levels: Array<Record<string, any>>;
}

export interface PerformanceResponse {
  range_key: string;
  history: Array<Record<string, any>>;
  goal_tracking: Record<string, any>;
}

export interface MarketResponse {
  items: Array<Record<string, any>>;
  summary: Record<string, any>;
}

export interface WatchlistCategory {
  key: string;
  label: string;
  target_limit: number;
  total_universe: number;
  items: Array<Record<string, any>>;
}

export interface WatchlistsResponse {
  generated_at: string;
  cache_ttl_seconds?: number;
  categories: WatchlistCategory[];
  nordic: Array<Record<string, any>>;
  uk: Array<Record<string, any>>;
  us: Array<Record<string, any>>;
  eu: Array<Record<string, any>>;
  global: Array<Record<string, any>>;
}

export interface DecisionResponse {
  report: Record<string, any> | null;
  next_report?: Record<string, any> | null;
}

export interface DecisionHistoryResponse {
  items: Array<Record<string, any>>;
}

export interface StrategyJournalResponse {
  items: Array<Record<string, any>>;
}

export interface PromptsResponse {
  generated_at: string;
  items: Array<Record<string, any>>;
  latest_decision_report?: Record<string, any> | null;
  latest_trading_manager_run?: Record<string, any> | null;
}

export interface ExecutionResponse {
  orders: Array<Record<string, any>>;
  fills: Array<Record<string, any>>;
  events: Array<Record<string, any>>;
}

export interface SchedulerResponse {
  status: Record<string, any> | null;
  cycles: Array<Record<string, any>>;
}
