import React from 'react';
import { cn } from '@/lib/utils';
import { RangePreset } from '@/hooks/useDateRange';

interface DateRangeSelectorProps {
  preset: RangePreset;
  onPresetChange: (preset: RangePreset) => void;
  onCustomRange: (from: Date, to: Date) => void;
}

const presetButtons: { label: string; value: RangePreset }[] = [
  { label: 'Today', value: 'today' },
  { label: '7d', value: '7d' },
  { label: '30d', value: '30d' },
  { label: '90d', value: '90d' },
  { label: 'Custom', value: 'custom' },
];

export function DateRangeSelector({ preset, onPresetChange, onCustomRange }: DateRangeSelectorProps) {
  const [showCustomPicker, setShowCustomPicker] = React.useState(preset === 'custom');
  const [customFrom, setCustomFrom] = React.useState('');
  const [customTo, setCustomTo] = React.useState('');

  const handleCustomApply = () => {
    if (customFrom && customTo) {
      const fromDate = new Date(customFrom);
      const toDate = new Date(customTo);
      toDate.setUTCHours(23, 59, 59, 999);
      onCustomRange(fromDate, toDate);
    }
  };

  const handleClick = (value: RangePreset) => {
    if (value === 'custom') {
      setShowCustomPicker(true);
    } else {
      setShowCustomPicker(false);
      onPresetChange(value);
    }
  };

  // Determine which button is visually active
  const activeButton = showCustomPicker && preset !== 'custom' ? null : preset;

  return (
    <div className="flex items-center gap-2">
      <div className="flex rounded-lg border border-[hsl(var(--border))] overflow-hidden">
        {presetButtons.map((p) => (
          <button
            key={p.value}
            onClick={() => handleClick(p.value)}
            className={cn(
              'px-3 py-1.5 text-sm font-medium transition-colors',
              (p.value === 'custom' ? showCustomPicker : activeButton === p.value)
                ? 'bg-[hsl(var(--primary))] text-[hsl(var(--primary-foreground))]'
                : 'hover:bg-[hsl(var(--muted))] text-[hsl(var(--muted-foreground))]'
            )}
          >
            {p.label}
          </button>
        ))}
      </div>
      {showCustomPicker && (
        <div className="flex items-center gap-2">
          <input
            type="date"
            value={customFrom}
            onChange={(e) => setCustomFrom(e.target.value)}
            className="border border-[hsl(var(--border))] rounded px-2 py-1 text-sm"
          />
          <span className="text-sm text-[hsl(var(--muted-foreground))]">to</span>
          <input
            type="date"
            value={customTo}
            onChange={(e) => setCustomTo(e.target.value)}
            className="border border-[hsl(var(--border))] rounded px-2 py-1 text-sm"
          />
          <button
            onClick={handleCustomApply}
            className="px-3 py-1.5 text-sm font-medium bg-[hsl(var(--primary))] text-[hsl(var(--primary-foreground))] rounded"
          >
            Apply
          </button>
        </div>
      )}
    </div>
  );
}
