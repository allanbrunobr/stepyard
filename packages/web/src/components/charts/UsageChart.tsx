import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from 'recharts';
import { DailyUsage } from '../../../../../types';

interface UsageChartProps {
  data: DailyUsage[];
}

export function UsageChart({ data }: UsageChartProps) {
  if (data.length === 0) {
    return (
      <div className="flex items-center justify-center h-64 text-[hsl(var(--muted-foreground))]">
        No usage data available
      </div>
    );
  }

  return (
    <ResponsiveContainer width="100%" height={300}>
      <AreaChart data={data} margin={{ top: 10, right: 30, left: 0, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" stroke="hsl(214.3, 31.8%, 91.4%)" />
        <XAxis
          dataKey="date"
          tick={{ fontSize: 12 }}
          tickFormatter={(val: string) => {
            const d = new Date(val);
            return `${d.getMonth() + 1}/${d.getDate()}`;
          }}
        />
        <YAxis tick={{ fontSize: 12 }} allowDecimals={false} />
        <Tooltip
          labelFormatter={(val: string) => new Date(val).toLocaleDateString()}
          formatter={(value: number) => [value, 'Workflows']}
        />
        <Area
          type="monotone"
          dataKey="count"
          stroke="hsl(221.2, 83.2%, 53.3%)"
          fill="hsl(221.2, 83.2%, 53.3%)"
          fillOpacity={0.15}
          strokeWidth={2}
        />
      </AreaChart>
    </ResponsiveContainer>
  );
}
