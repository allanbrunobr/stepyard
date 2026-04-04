// Shared types for the minion-engine dashboard

export interface WorkflowRun {
  run_id: string;
  user_name: string;
  workflow: string;
  target?: string;
  repo?: string;
  status: string;
  duration_ms?: number;
  total_tokens?: number;
  cost_usd?: number;
  started_at: string;
  finished_at?: string;
  error?: string;
  event_version?: number;
  created_at?: string;
  updated_at?: string;
}

export interface WorkflowStep {
  id?: number | string;
  run_id: string;
  step_name: string;
  step_type?: string;
  status: string;
  duration_ms?: number;
  tokens_in?: number;
  tokens_out?: number;
  tokens_used?: number;
  sandboxed?: boolean;
  cost_usd?: number;
  started_at?: string;
  finished_at?: string | null;
  error_message?: string | null;
}

export interface EventPayload {
  run_id: string;
  user_name: string;
  workflow: string;
  target?: string;
  repo?: string;
  status: string;
  duration_ms?: number;
  total_tokens?: number;
  cost_usd?: number;
  started_at: string;
  finished_at?: string;
  error?: string;
  event_version?: number;
  steps?: WorkflowStep[];
}

export interface HealthResponse {
  status: string;
}

export interface OverviewSummary {
  total_runs: number;
  total_tokens: number;
  total_cost_usd: number;
  active_developers: number;
}

export interface DailyUsage {
  date: string;
  count: number;
}

export interface PeakHour {
  hour: number;
  count: number;
}

export interface DeveloperRanking {
  user_name: string;
  workflow_count: number;
  total_tokens: number;
  total_cost_usd: number;
}

export interface CostByDeveloper {
  user_name: string;
  cost_usd: number;
  total_tokens: number;
  run_count: number;
}

export interface CostByWorkflow {
  workflow: string;
  cost_usd: number;
  total_tokens: number;
  run_count: number;
}

export interface CostByRepo {
  repo: string;
  cost_usd: number;
  total_tokens: number;
  run_count: number;
}

export interface DailyCost {
  date: string;
  cost_usd: number;
}

export interface DateRangeParams {
  from: string;
  to: string;
}
