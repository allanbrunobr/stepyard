export interface WorkflowRun {
  id: string;
  run_id: string;
  workflow_name: string;
  user_name: string;
  status: 'running' | 'completed' | 'failed' | 'cancelled';
  total_tokens: number;
  cost_usd: number;
  started_at: string;
  finished_at: string | null;
  error_message: string | null;
}

export interface WorkflowStep {
  id: string;
  run_id: string;
  step_name: string;
  step_type: string;
  status: 'running' | 'completed' | 'failed' | 'skipped';
  tokens_used: number;
  cost_usd: number;
  started_at: string;
  finished_at: string | null;
  error_message: string | null;
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
