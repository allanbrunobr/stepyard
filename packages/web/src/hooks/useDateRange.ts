import { useState, useMemo } from 'react';

export type RangePreset = 'today' | '7d' | '30d' | '90d' | 'custom';

export interface DateRange {
  from: Date;
  to: Date;
  preset: RangePreset;
  setPreset: (preset: RangePreset) => void;
  setCustomRange: (from: Date, to: Date) => void;
}

function getPresetDates(preset: RangePreset): { from: Date; to: Date } {
  const now = new Date();
  const to = now;

  switch (preset) {
    case 'today': {
      const from = new Date(now);
      from.setUTCHours(0, 0, 0, 0);
      return { from, to };
    }
    case '7d':
      return { from: new Date(now.getTime() - 7 * 24 * 60 * 60 * 1000), to };
    case '30d':
      return { from: new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000), to };
    case '90d':
      return { from: new Date(now.getTime() - 90 * 24 * 60 * 60 * 1000), to };
    case 'custom':
      return { from: new Date(now.getTime() - 30 * 24 * 60 * 60 * 1000), to };
  }
}

export function useDateRange(): DateRange {
  const [preset, setPreset] = useState<RangePreset>('30d');
  const [customFrom, setCustomFrom] = useState<Date | null>(null);
  const [customTo, setCustomTo] = useState<Date | null>(null);

  const { from, to } = useMemo(() => {
    if (preset === 'custom' && customFrom && customTo) {
      return { from: customFrom, to: customTo };
    }
    return getPresetDates(preset);
  }, [preset, customFrom, customTo]);

  const setCustomRange = (from: Date, to: Date) => {
    setCustomFrom(from);
    setCustomTo(to);
    setPreset('custom');
  };

  return { from, to, preset, setPreset, setCustomRange };
}
