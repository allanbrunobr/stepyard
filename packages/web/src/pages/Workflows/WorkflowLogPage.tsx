import { useSearchParams } from 'react-router-dom';
import { Download } from 'lucide-react';
import { PageLayout } from '@/components/layout/PageLayout';
import { Button } from '@/components/ui/button';
import { WorkflowFilters } from '@/components/workflow/WorkflowFilters';
import { WorkflowTable } from '@/components/workflow/WorkflowTable';
import { Pagination } from '@/components/workflow/Pagination';
import { useWorkflows } from '@/hooks/use-workflows';

export function WorkflowLogPage() {
  const [searchParams, setSearchParams] = useSearchParams();

  const filters = {
    page: parseInt(searchParams.get('page') ?? '1', 10),
    limit: parseInt(searchParams.get('limit') ?? '25', 10),
    sort: searchParams.get('sort') ?? 'started_at',
    order: searchParams.get('order') ?? 'desc',
    user_name: searchParams.get('user_name') ?? '',
    workflow: searchParams.get('workflow') ?? '',
    status: searchParams.get('status') ?? '',
    from: searchParams.get('from') ?? '',
    to: searchParams.get('to') ?? '',
  };

  const { data, isLoading, error } = useWorkflows({
    ...filters,
    user_name: filters.user_name || undefined,
    workflow: filters.workflow || undefined,
    status: filters.status || undefined,
    from: filters.from || undefined,
    to: filters.to || undefined,
  });

  function updateParam(key: string, value: string) {
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      if (value) {
        next.set(key, value);
      } else {
        next.delete(key);
      }
      if (key !== 'page') next.set('page', '1');
      return next;
    });
  }

  function handleSort(column: string) {
    const newOrder = filters.sort === column && filters.order === 'desc' ? 'asc' : 'desc';
    setSearchParams((prev) => {
      const next = new URLSearchParams(prev);
      next.set('sort', column);
      next.set('order', newOrder);
      next.set('page', '1');
      return next;
    });
  }

  function handleReset() {
    setSearchParams({});
  }

  function handleExport() {
    const params = new URLSearchParams();
    if (filters.user_name) params.set('user_name', filters.user_name);
    if (filters.workflow) params.set('workflow', filters.workflow);
    if (filters.status) params.set('status', filters.status);
    if (filters.from) params.set('from', filters.from);
    if (filters.to) params.set('to', filters.to);
    window.location.href = `/api/workflows/export?${params.toString()}`;
  }

  return (
    <PageLayout>
      <div className="flex items-center justify-between mb-6">
        <h1 className="text-2xl font-bold">Workflow Logs</h1>
        <Button variant="outline" size="sm" onClick={handleExport}>
          <Download className="h-4 w-4 mr-2" />
          Export CSV
        </Button>
      </div>

      <WorkflowFilters
        filters={{
          user_name: filters.user_name,
          workflow: filters.workflow,
          status: filters.status,
          from: filters.from,
          to: filters.to,
        }}
        onChange={updateParam}
        onReset={handleReset}
      />

      {error && (
        <div className="rounded-md bg-destructive/10 text-destructive p-3 mb-4 text-sm">
          Failed to load workflows: {error.message}
        </div>
      )}

      {isLoading ? (
        <div className="text-center py-12 text-muted-foreground">Loading...</div>
      ) : (
        <>
          <WorkflowTable
            data={data?.data ?? []}
            sort={filters.sort}
            order={filters.order}
            onSort={handleSort}
          />
          {data?.meta && (
            <Pagination
              page={data.meta.page}
              totalPages={data.meta.totalPages}
              total={data.meta.total}
              onPageChange={(p) => updateParam('page', String(p))}
            />
          )}
        </>
      )}
    </PageLayout>
  );
}
