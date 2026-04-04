import { Badge } from '@/components/ui/badge';
import { StatusBadge } from '@/components/shared/StatusBadge';
import { Shield } from 'lucide-react';
import { formatDuration } from '@/lib/format';
import type { WorkflowStep } from '../../types';

const stepTypeLabels: Record<string, string> = {
  cmd: 'Command',
  chat: 'Chat',
  agent: 'Agent',
  gate: 'Gate',
  map: 'Map',
};

interface StepTimelineProps {
  steps: WorkflowStep[];
}

export function StepTimeline({ steps }: StepTimelineProps) {
  if (steps.length === 0) {
    return <p className="text-muted-foreground text-sm py-4">No steps recorded for this run.</p>;
  }

  return (
    <div className="relative ml-4">
      <div className="absolute left-3 top-0 bottom-0 w-px bg-border" />
      <div className="flex flex-col gap-3">
        {steps.map((step, index) => {
          const isFailed = step.status === 'failed';
          return (
            <div
              key={step.id ?? index}
              className={`relative pl-10 py-3 pr-4 rounded-md border ${isFailed ? 'border-destructive bg-destructive/5' : 'border-border bg-card'}`}
            >
              <div
                className={`absolute left-1.5 top-5 h-3 w-3 rounded-full border-2 ${isFailed ? 'border-destructive bg-destructive' : step.status === 'success' ? 'border-green-500 bg-green-500' : 'border-yellow-500 bg-yellow-500'}`}
              />

              <div className="flex items-center justify-between gap-2 mb-1">
                <div className="flex items-center gap-2">
                  <span className="font-medium text-sm">{step.step_name}</span>
                  {step.step_type && (
                    <Badge variant="secondary" className="text-xs">
                      {stepTypeLabels[step.step_type] ?? step.step_type}
                    </Badge>
                  )}
                  <StatusBadge status={step.status} />
                  {step.sandboxed && (
                    <span className="inline-flex items-center gap-1 text-xs text-muted-foreground" title="Sandboxed">
                      <Shield className="h-3 w-3" /> Sandbox
                    </span>
                  )}
                </div>
                <div className="flex items-center gap-4 text-xs text-muted-foreground">
                  {step.duration_ms != null && <span>{formatDuration(step.duration_ms)}</span>}
                  {step.tokens_in != null && <span>{step.tokens_in.toLocaleString()} in</span>}
                  {step.tokens_out != null && <span>{step.tokens_out.toLocaleString()} out</span>}
                </div>
              </div>

              {isFailed && step.status === 'failed' && (
                <p className="text-destructive text-xs mt-1">Step failed</p>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
