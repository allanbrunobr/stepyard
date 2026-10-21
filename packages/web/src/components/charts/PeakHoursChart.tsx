import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
  Cell,
} from 'recharts';
import { PeakHour } from '../../../../../types';

interface PeakHoursChartProps {
  data: PeakHour[];
}

function getColorIntensity(count: number, maxCount: number): string {
  if (maxCount === 0) return 'hsl(221.2, 83.2%, 93%)';
  const ratio = count / maxCount;
  // Interpolate lightness from 93% (light) to 53% (dark)
  const lightness = 93 - ratio * 40;
  return `hsl(221.2, 83.2%, ${lightness}%)`;
}

export function PeakHoursChart({ data }: PeakHoursChartProps) {
  if (data.length === 0) {
    return (
      <div className="flex items-center justify-center h-64 text-[hsl(var(--muted-foreground))]">
        No peak hours data available
      </div>
    );
  }

  const maxCount = Math.max(...data.map((d) => d.count), 1);

  return (
    <ResponsiveContainer width="100%" height={300}>
      <BarChart data={data} margin={{ top: 10, right: 30, left: 0, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" stroke="hsl(214.3, 31.8%, 91.4%)" />
        <XAxis
          dataKey="hour"
          tick={{ fontSize: 12 }}
          tickFormatter={(val: number) => `${val}:00`}
        />
        <YAxis tick={{ fontSize: 12 }} allowDecimals={false} />
        <Tooltip
          labelFormatter={(val: number) => `${val}:00 - ${val}:59`}
          formatter={(value: number) => [value, 'Workflows']}
        />
        <Bar dataKey="count" radius={[4, 4, 0, 0]}>
          {data.map((entry, index) => (
            <Cell key={index} fill={getColorIntensity(entry.count, maxCount)} />
          ))}
        </Bar>
      </BarChart>
    </ResponsiveContainer>
  );
}
