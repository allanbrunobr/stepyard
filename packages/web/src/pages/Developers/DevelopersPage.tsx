import { useEffect, useState, useCallback } from "react";
import { useNavigate } from "react-router-dom";
import { PageHeader } from "../../components/layout";
import {
  DateRangePicker,
  LoadingState,
  EmptyState,
} from "../../components/analytics";
import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "../../components/ui/table";
import { Card, CardContent, CardHeader, CardTitle } from "../../components/ui/card";
import { apiFetch } from "../../lib/api-client";
import { formatUsd, formatNumber } from "../../lib/utils";
import type { DeveloperRanking } from "../../../../types";

function toLocalDateStr(d: Date): string {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function defaultFrom(): string {
  const d = new Date();
  d.setDate(d.getDate() - 30);
  return toLocalDateStr(d);
}

function defaultTo(): string {
  return toLocalDateStr(new Date());
}

export function DevelopersPage() {
  const navigate = useNavigate();
  const [from, setFrom] = useState(defaultFrom);
  const [to, setTo] = useState(defaultTo);
  const [data, setData] = useState<DeveloperRanking[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const fetchData = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await apiFetch<DeveloperRanking[]>(
        `/analytics/developers?from=${from}&to=${to}`
      );
      setData(result);
    } catch {
      setData([]);
      setError("Failed to load developer data. Please try again.");
    } finally {
      setLoading(false);
    }
  }, [from, to]);

  useEffect(() => {
    void fetchData();
  }, [fetchData]);

  return (
    <div>
      <PageHeader title="Developer Activity" description="Developer rankings by AI workflow usage">
        <DateRangePicker
          from={from}
          to={to}
          onFromChange={setFrom}
          onToChange={setTo}
        />
      </PageHeader>

      {error && (
        <div className="mb-4 rounded-md border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-800">
          {error}
        </div>
      )}

      <Card>
        <CardHeader>
          <CardTitle>Developer Rankings</CardTitle>
        </CardHeader>
        <CardContent>
          {loading ? (
            <LoadingState />
          ) : data.length === 0 && !error ? (
            <EmptyState />
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="w-16">Rank</TableHead>
                  <TableHead>Developer Name</TableHead>
                  <TableHead className="text-right">Workflow Count</TableHead>
                  <TableHead className="text-right">Total Tokens</TableHead>
                  <TableHead className="text-right">Total Cost (USD)</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {data.map((dev, index) => (
                  <TableRow key={dev.user_name}>
                    <TableCell className="font-medium">{index + 1}</TableCell>
                    <TableCell>
                      <button
                        onClick={() =>
                          navigate(
                            `/workflows?developer=${encodeURIComponent(dev.user_name)}`
                          )
                        }
                        className="text-primary hover:underline font-medium"
                      >
                        {dev.user_name}
                      </button>
                    </TableCell>
                    <TableCell className="text-right">
                      {formatNumber(dev.workflow_count)}
                    </TableCell>
                    <TableCell className="text-right">
                      {formatNumber(dev.total_tokens)}
                    </TableCell>
                    <TableCell className="text-right">
                      {formatUsd(dev.total_cost_usd)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
