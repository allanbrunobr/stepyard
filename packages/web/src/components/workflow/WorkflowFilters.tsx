import { Select } from '@/components/ui/select';
import { Input } from '@/components/ui/input';
import { Button } from '@/components/ui/button';
import { useWorkflowDistinctValues } from '@/hooks/use-workflows';

interface WorkflowFiltersProps {
  filters: {
    user_name: string;
    workflow: string;
    status: string;
    from: string;
    to: string;
  };
  onChange: (key: string, value: string) => void;
  onReset: () => void;
}

export function WorkflowFilters({ filters, onChange, onReset }: WorkflowFiltersProps) {
  const { data: distinctData } = useWorkflowDistinctValues();
  const distinct = distinctData?.data;

  const userOptions = (distinct?.user_names ?? []).map((v) => ({ value: v, label: v }));
  const workflowOptions = (distinct?.workflows ?? []).map((v) => ({ value: v, label: v }));
  const statusOptions = (distinct?.statuses ?? []).map((v) => ({ value: v, label: v.charAt(0).toUpperCase() + v.slice(1) }));

  const hasActiveFilters = Object.values(filters).some((v) => v !== '');

  return (
    <div className="flex flex-wrap items-end gap-3 mb-4">
      <div className="flex flex-col gap-1">
        <label className="text-xs text-muted-foreground">Developer</label>
        <Select
          options={userOptions}
          placeholder="All developers"
          value={filters.user_name}
          onChange={(e) => onChange('user_name', e.target.value)}
          className="w-44"
        />
      </div>
      <div className="flex flex-col gap-1">
        <label className="text-xs text-muted-foreground">Workflow</label>
        <Select
          options={workflowOptions}
          placeholder="All workflows"
          value={filters.workflow}
          onChange={(e) => onChange('workflow', e.target.value)}
          className="w-44"
        />
      </div>
      <div className="flex flex-col gap-1">
        <label className="text-xs text-muted-foreground">Status</label>
        <Select
          options={statusOptions}
          placeholder="All statuses"
          value={filters.status}
          onChange={(e) => onChange('status', e.target.value)}
          className="w-36"
        />
      </div>
      <div className="flex flex-col gap-1">
        <label className="text-xs text-muted-foreground">From</label>
        <Input
          type="date"
          value={filters.from}
          onChange={(e) => onChange('from', e.target.value)}
          className="w-40"
        />
      </div>
      <div className="flex flex-col gap-1">
        <label className="text-xs text-muted-foreground">To</label>
        <Input
          type="date"
          value={filters.to}
          onChange={(e) => onChange('to', e.target.value)}
          className="w-40"
        />
      </div>
      {hasActiveFilters && (
        <Button variant="ghost" size="sm" onClick={onReset}>
          Clear filters
        </Button>
      )}
    </div>
  );
}
