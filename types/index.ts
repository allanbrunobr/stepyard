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
  created_at?: string;
  updated_at?: string;
}

export interface WorkflowStep {
  id?: number;
  run_id: string;
  step_name: string;
  step_type?: string;
  status: string;
  duration_ms?: number;
  tokens_in?: number;
  tokens_out?: number;
  sandboxed?: boolean;
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
  steps?: WorkflowStep[];
}

export interface HealthResponse {
  status: string;
}
