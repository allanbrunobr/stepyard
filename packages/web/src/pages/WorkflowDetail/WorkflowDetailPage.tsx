import { useParams, useNavigate, useLocation } from 'react-router-dom';
import { ArrowLeft } from 'lucide-react';
import { PageLayout } from '@/components/layout/PageLayout';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { StatusBadge } from '@/components/shared/StatusBadge';
import { StepTimeline } from '@/components/workflow/StepTimeline';
import { useWorkflowDetail } from '@/hooks/use-workflows';
import { formatTimestamp, formatDuration, formatCost } from '@/lib/format';

export function WorkflowDetailPage() {
  const { runId } = useParams<{ runId: string }>();
  const navigate = useNavigate();
  const location = useLocation();

  const { data, isLoading, error } = useWorkflowDetail(runId!);
  const run = data?.data;

  function handleBack() {
    // If we came from the workflow list, go back to preserve full browser history.
    // The search params were forwarded from the list page into this URL.
    if (window.history.length > 1) {
      navigate(-1);
    } else {
      // Direct navigation fallback: use forwarded search params from the URL
      navigate(`/workflows${location.search}`);
    }
  }

  return (
    <PageLayout>
      <Button variant="ghost" size="sm" onClick={handleBack} className="mb-4">
        <ArrowLeft className="h-4 w-4 mr-2" />
        Back to Workflows
      </Button>

      {isLoading && (
        <div className="text-center py-12 text-muted-foreground">Loading...</div>
      )}

      {error && (
        <div className="rounded-md bg-destructive/10 text-destructive p-3 text-sm">
          Failed to load workflow: {error.message}
        </div>
      )}

      {run && (
        <>
          <Card className="mb-6">
            <CardHeader>
              <div className="flex items-center justify-between">
                <CardTitle className="text-xl">Run Details</CardTitle>
                <StatusBadge status={run.status} />
              </div>
            </CardHeader>
            <CardContent>
              <dl className="grid grid-cols-2 md:grid-cols-4 gap-4 text-sm">
                <div>
                  <dt className="text-muted-foreground">Run ID</dt>
                  <dd className="font-mono text-xs break-all">{run.run_id}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Developer</dt>
                  <dd>{run.user_name}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Workflow</dt>
                  <dd>{run.workflow}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Target</dt>
                  <dd>{run.target ?? '-'}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Repository</dt>
                  <dd>{run.repo ?? '-'}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Duration</dt>
                  <dd>{run.duration_ms != null ? formatDuration(run.duration_ms) : '-'}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Total Tokens</dt>
                  <dd>{run.total_tokens?.toLocaleString() ?? '-'}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Cost</dt>
                  <dd>{run.cost_usd != null ? formatCost(run.cost_usd) : '-'}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Started</dt>
                  <dd>{formatTimestamp(run.started_at)}</dd>
                </div>
                <div>
                  <dt className="text-muted-foreground">Finished</dt>
                  <dd>{run.finished_at ? formatTimestamp(run.finished_at) : '-'}</dd>
                </div>
              </dl>

              {run.error && (
                <div className="mt-4 rounded-md bg-destructive/10 text-destructive p-3 text-sm">
                  <strong>Error:</strong> {run.error}
                </div>
              )}
            </CardContent>
          </Card>

          <h2 className="text-lg font-semibold mb-4">Execution Steps</h2>
          <StepTimeline steps={run.steps ?? []} />
        </>
      )}
    </PageLayout>
  );
}
