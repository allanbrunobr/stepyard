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

function defaultFrom(): string {
  const d = new Date();
  d.setDate(d.getDate() - 30);
  return d.toISOString().split("T")[0];
}

function defaultTo(): string {
  return new Date().toISOString().split("T")[0];
}

export function DevelopersPage() {
  const navigate = useNavigate();
  const [from, setFrom] = useState(defaultFrom);
  const [to, setTo] = useState(defaultTo);
  const [data, setData] = useState<DeveloperRanking[]>([]);
  const [loading, setLoading] = useState(true);

  const fetchData = useCallback(async () => {
    setLoading(true);
    try {
      const result = await apiFetch<DeveloperRanking[]>(
        `/analytics/developers?from=${from}&to=${to}`
      );
      setData(result);
    } catch {
      setData([]);
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

      <Card>
        <CardHeader>
          <CardTitle>Developer Rankings</CardTitle>
        </CardHeader>
        <CardContent>
          {loading ? (
            <LoadingState />
          ) : data.length === 0 ? (
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
