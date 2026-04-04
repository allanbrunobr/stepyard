// Shared types for the minion-engine dashboard

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
