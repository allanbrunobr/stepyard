import { useQuery } from '@tanstack/react-query';
import { apiFetch } from '@/lib/api-client';
import type { WorkflowRun, WorkflowStep } from '../types';

interface PaginationMeta {
  total: number;
  page: number;
  limit: number;
  totalPages: number;
}

interface WorkflowListResponse {
  data: WorkflowRun[];
  meta: PaginationMeta;
}

interface WorkflowDetailResponse {
  data: WorkflowRun & { steps: WorkflowStep[] };
}

interface DistinctValuesResponse {
  data: {
    user_names: string[];
    workflows: string[];
    statuses: string[];
  };
}

export interface WorkflowFilters {
  page?: number;
  limit?: number;
  sort?: string;
  order?: string;
  user_name?: string;
  workflow?: string;
  status?: string;
  from?: string;
  to?: string;
}

function buildQueryString(filters: WorkflowFilters): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(filters)) {
    if (value !== undefined && value !== '') {
      params.set(key, String(value));
    }
  }
  return params.toString();
}

export function useWorkflows(filters: WorkflowFilters) {
  const qs = buildQueryString(filters);
  return useQuery<WorkflowListResponse>({
    queryKey: ['workflows', qs],
    queryFn: () => apiFetch<WorkflowListResponse>(`/workflows?${qs}`),
  });
}

export function useWorkflowDetail(runId: string) {
  return useQuery<WorkflowDetailResponse>({
    queryKey: ['workflow', runId],
    queryFn: () => apiFetch<WorkflowDetailResponse>(`/workflows/${runId}`),
    enabled: !!runId,
  });
}

export function useWorkflowDistinctValues() {
  return useQuery<DistinctValuesResponse>({
    queryKey: ['workflows', 'distinct'],
    queryFn: () => apiFetch<DistinctValuesResponse>('/workflows/distinct'),
    staleTime: 60_000,
  });
}
