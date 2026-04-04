import { useNavigate } from 'react-router-dom';
import { ArrowUpDown, ArrowUp, ArrowDown } from 'lucide-react';
import { StatusBadge } from '@/components/shared/StatusBadge';
import type { WorkflowRun } from '../../types';
import { formatDuration, formatCost, formatTimestamp } from '@/lib/format';

interface Column {
  key: string;
  label: string;
  sortable: boolean;
}

const columns: Column[] = [
  { key: 'started_at', label: 'Timestamp', sortable: true },
  { key: 'user_name', label: 'Developer', sortable: true },
  { key: 'workflow', label: 'Workflow', sortable: true },
  { key: 'target', label: 'Target', sortable: false },
  { key: 'repo', label: 'Repository', sortable: false },
  { key: 'status', label: 'Status', sortable: true },
  { key: 'duration_ms', label: 'Duration', sortable: true },
  { key: 'total_tokens', label: 'Tokens', sortable: true },
  { key: 'cost_usd', label: 'Cost', sortable: true },
];

interface WorkflowTableProps {
  data: WorkflowRun[];
  sort: string;
  order: string;
  onSort: (column: string) => void;
}

export function WorkflowTable({ data, sort, order, onSort }: WorkflowTableProps) {
  const navigate = useNavigate();

  function getSortIcon(columnKey: string) {
    if (sort !== columnKey) return <ArrowUpDown className="h-3 w-3 ml-1 opacity-40" />;
    return order === 'asc'
      ? <ArrowUp className="h-3 w-3 ml-1" />
      : <ArrowDown className="h-3 w-3 ml-1" />;
  }

  function renderCell(row: WorkflowRun, key: string) {
    switch (key) {
      case 'started_at':
        return formatTimestamp(row.started_at);
      case 'status':
        return <StatusBadge status={row.status} />;
      case 'duration_ms':
        return row.duration_ms != null ? formatDuration(row.duration_ms) : '-';
      case 'total_tokens':
        return row.total_tokens?.toLocaleString() ?? '-';
      case 'cost_usd':
        return row.cost_usd != null ? formatCost(row.cost_usd) : '-';
      default:
        return (row as unknown as Record<string, unknown>)[key] as string ?? '-';
    }
  }

  return (
    <div className="rounded-md border overflow-auto">
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b bg-muted/50">
            {columns.map((col) => (
              <th
                key={col.key}
                className={`h-10 px-4 text-left font-medium text-muted-foreground ${col.sortable ? 'cursor-pointer select-none hover:text-foreground' : ''}`}
                onClick={() => col.sortable && onSort(col.key)}
              >
                <span className="flex items-center">
                  {col.label}
                  {col.sortable && getSortIcon(col.key)}
                </span>
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {data.length === 0 ? (
            <tr>
              <td colSpan={columns.length} className="h-24 text-center text-muted-foreground">
                No workflows found.
              </td>
            </tr>
          ) : (
            data.map((row) => (
              <tr
                key={row.run_id}
                className="border-b hover:bg-muted/50 cursor-pointer transition-colors"
                onClick={() => navigate(`/workflows/${row.run_id}${window.location.search}`)}
              >
                {columns.map((col) => (
                  <td key={col.key} className="px-4 py-3">
                    {renderCell(row, col.key)}
                  </td>
                ))}
              </tr>
            ))
          )}
        </tbody>
      </table>
    </div>
  );
}
