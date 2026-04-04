import { useState, useEffect, useCallback, useRef } from 'react';
import { apiFetch } from '@/lib/api-client';
import { OverviewSummary, DailyUsage, PeakHour } from '../../../../types';

interface OverviewData {
  summary: OverviewSummary | null;
  dailyUsage: DailyUsage[];
  peakHours: PeakHour[];
  loading: boolean;
  error: string | null;
  refresh: () => void;
}

export function useOverviewData(from: Date, to: Date): OverviewData {
  const [summary, setSummary] = useState<OverviewSummary | null>(null);
  const [dailyUsage, setDailyUsage] = useState<DailyUsage[]>([]);
  const [peakHours, setPeakHours] = useState<PeakHour[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const abortControllerRef = useRef<AbortController | null>(null);

  const fetchData = useCallback(async () => {
    // Cancel any in-flight request
    if (abortControllerRef.current) {
      abortControllerRef.current.abort();
    }
    const controller = new AbortController();
    abortControllerRef.current = controller;

    setLoading(true);
    setError(null);

    const params = `from=${from.toISOString()}&to=${to.toISOString()}`;

    try {
      const [summaryData, usageData, hoursData] = await Promise.all([
        apiFetch<OverviewSummary>(`/overview/summary?${params}`, { signal: controller.signal }),
        apiFetch<DailyUsage[]>(`/overview/daily-usage?${params}`, { signal: controller.signal }),
        apiFetch<PeakHour[]>(`/overview/peak-hours?${params}`, { signal: controller.signal }),
      ]);

      if (!controller.signal.aborted) {
        setSummary(summaryData);
        setDailyUsage(usageData);
        setPeakHours(hoursData);
      }
    } catch (err) {
      if (err instanceof DOMException && err.name === 'AbortError') return;
      setError(err instanceof Error ? err.message : 'Failed to fetch data');
    } finally {
      if (!controller.signal.aborted) {
        setLoading(false);
      }
    }
  }, [from.toISOString(), to.toISOString()]);

  useEffect(() => {
    fetchData();
    return () => {
      abortControllerRef.current?.abort();
    };
  }, [fetchData]);

  return { summary, dailyUsage, peakHours, loading, error, refresh: fetchData };
}
