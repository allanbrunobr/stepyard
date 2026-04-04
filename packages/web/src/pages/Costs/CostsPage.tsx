import { useEffect, useState, useCallback } from "react";
import {
  LineChart,
  Line,
  XAxis,
  YAxis,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import { PageHeader } from "../../components/layout";
import {
  DateRangePicker,
  LoadingState,
  EmptyState,
} from "../../components/analytics";
import { CostTable } from "../../components/analytics/CostTable";
import { Card, CardContent, CardHeader, CardTitle } from "../../components/ui/card";
import { apiFetch } from "../../lib/api-client";
import { formatUsd } from "../../lib/utils";
import type {
  CostByDeveloper,
  CostByWorkflow,
  CostByRepo,
  DailyCost,
} from "../../../../types";

function defaultFrom(): string {
  const d = new Date();
  d.setDate(d.getDate() - 30);
  return d.toISOString().split("T")[0];
}

function defaultTo(): string {
  return new Date().toISOString().split("T")[0];
}

export function CostsPage() {
  const [from, setFrom] = useState(defaultFrom);
  const [to, setTo] = useState(defaultTo);
  const [loading, setLoading] = useState(true);
  const [byDeveloper, setByDeveloper] = useState<CostByDeveloper[]>([]);
  const [byWorkflow, setByWorkflow] = useState<CostByWorkflow[]>([]);
  const [byRepo, setByRepo] = useState<CostByRepo[]>([]);
  const [daily, setDaily] = useState<DailyCost[]>([]);

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      const qs = `from=${from}&to=${to}`;
      const [devData, wfData, repoData, dailyData] = await Promise.all([
        apiFetch<CostByDeveloper[]>(`/analytics/costs/by-developer?${qs}`),
        apiFetch<CostByWorkflow[]>(`/analytics/costs/by-workflow?${qs}`),
        apiFetch<CostByRepo[]>(`/analytics/costs/by-repo?${qs}`),
        apiFetch<DailyCost[]>(`/analytics/costs/daily?${qs}`),
      ]);
      setByDeveloper(devData);
      setByWorkflow(wfData);
      setByRepo(repoData);
      setDaily(dailyData);
    } catch {
      setByDeveloper([]);
      setByWorkflow([]);
      setByRepo([]);
      setDaily([]);
    } finally {
      setLoading(false);
    }
  }, [from, to]);

  useEffect(() => {
    void fetchData();
  }, [fetchData]);

  const hasData =
    byDeveloper.length > 0 ||
    byWorkflow.length > 0 ||
    byRepo.length > 0 ||
    daily.length > 0;

  return (
    <div>
      <PageHeader title="Cost Tracking" description="AI spend breakdowns and trends">
        <DateRangePicker
          from={from}
          to={to}
          onFromChange={setFrom}
          onToChange={setTo}
        />
      </PageHeader>

      {loading ? (
        <LoadingState />
      ) : !hasData ? (
        <EmptyState />
      ) : (
        <div className="space-y-6">
          <Card>
            <CardHeader>
              <CardTitle>Daily Cost Trend</CardTitle>
            </CardHeader>
            <CardContent>
              {daily.length === 0 ? (
                <EmptyState message="No daily cost data available." />
              ) : (
                <ResponsiveContainer width="100%" height={300}>
                  <LineChart data={daily}>
                    <XAxis
                      dataKey="date"
                      tick={{ fontSize: 12 }}
                      label={{ value: "Date", position: "insideBottom", offset: -5 }}
                    />
                    <YAxis
                      tick={{ fontSize: 12 }}
                      tickFormatter={(v: number) => `$${v.toFixed(0)}`}
                      label={{
                        value: "Cost (USD)",
                        angle: -90,
                        position: "insideLeft",
                        offset: 10,
                      }}
                    />
                    <Tooltip
                      formatter={(value: number) => [formatUsd(value), "Cost"]}
                      labelFormatter={(label: string) => `Date: ${label}`}
                    />
                    <Line
                      type="monotone"
                      dataKey="cost_usd"
                      stroke="hsl(222.2 47.4% 11.2%)"
                      strokeWidth={2}
                      dot={{ r: 3 }}
                    />
                  </LineChart>
                </ResponsiveContainer>
              )}
            </CardContent>
          </Card>

          <div className="grid gap-6 md:grid-cols-1 lg:grid-cols-3">
            <Card>
              <CardContent className="pt-6">
                <CostTable
                  title="Cost by Developer"
                  labelHeader="Developer"
                  rows={byDeveloper.map((r) => ({
                    label: r.user_name,
                    cost_usd: r.cost_usd,
                    total_tokens: r.total_tokens,
                    run_count: r.run_count,
                  }))}
                />
              </CardContent>
            </Card>

            <Card>
              <CardContent className="pt-6">
                <CostTable
                  title="Cost by Workflow Type"
                  labelHeader="Workflow"
                  rows={byWorkflow.map((r) => ({
                    label: r.workflow,
                    cost_usd: r.cost_usd,
                    total_tokens: r.total_tokens,
                    run_count: r.run_count,
                  }))}
                />
              </CardContent>
            </Card>

            <Card>
              <CardContent className="pt-6">
                <CostTable
                  title="Cost by Repository"
                  labelHeader="Repository"
                  rows={byRepo.map((r) => ({
                    label: r.repo,
                    cost_usd: r.cost_usd,
                    total_tokens: r.total_tokens,
                    run_count: r.run_count,
                  }))}
                />
              </CardContent>
            </Card>
          </div>
        </div>
      )}
    </div>
  );
}
