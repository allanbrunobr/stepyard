import { Card, CardHeader, CardTitle, CardContent } from '@/components/ui/card';
import { DateRangeSelector } from '@/components/ui/DateRangeSelector';
import { UsageChart } from '@/components/charts/UsageChart';
import { PeakHoursChart } from '@/components/charts/PeakHoursChart';
import { useOverviewData } from '@/hooks/useOverviewData';
import { useDateRange } from '@/hooks/useDateRange';
import { useAutoRefresh } from '@/hooks/useAutoRefresh';
import { formatTokens, formatCost } from '@/lib/utils';

export function OverviewPage() {
  const dateRange = useDateRange();
  const { summary, dailyUsage, peakHours, loading, error, refresh } = useOverviewData(
    dateRange.from,
    dateRange.to
  );

  useAutoRefresh(refresh, 30000);

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-between">
        <h2 className="text-2xl font-bold">Overview</h2>
        <DateRangeSelector
          preset={dateRange.preset}
          onPresetChange={dateRange.setPreset}
          onCustomRange={dateRange.setCustomRange}
        />
      </div>

      {error && (
        <div className="bg-red-50 border border-red-200 text-red-700 px-4 py-3 rounded">
          {error}
        </div>
      )}

      {/* Summary Cards */}
      <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-4">
        <Card>
          <CardHeader>
            <CardTitle>Total Workflows</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">
              {loading ? '...' : (summary?.total_runs ?? 0).toLocaleString()}
            </p>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Total Tokens</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">
              {loading ? '...' : formatTokens(summary?.total_tokens ?? 0)}
            </p>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Estimated Cost</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">
              {loading ? '...' : formatCost(summary?.total_cost_usd ?? 0)}
            </p>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle>Active Developers</CardTitle>
          </CardHeader>
          <CardContent>
            <p className="text-3xl font-bold">
              {loading ? '...' : (summary?.active_developers ?? 0).toLocaleString()}
            </p>
          </CardContent>
        </Card>
      </div>

      {/* Daily Usage Chart */}
      <Card>
        <CardHeader>
          <CardTitle>Daily Usage</CardTitle>
        </CardHeader>
        <CardContent>
          <UsageChart data={dailyUsage} />
        </CardContent>
      </Card>

      {/* Peak Hours Chart */}
      <Card>
        <CardHeader>
          <CardTitle>Peak Hours</CardTitle>
        </CardHeader>
        <CardContent>
          <PeakHoursChart data={peakHours} />
        </CardContent>
      </Card>
    </div>
  );
}
