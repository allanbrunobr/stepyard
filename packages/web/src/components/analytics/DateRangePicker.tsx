interface DateRangePickerProps {
  from: string;
  to: string;
  onFromChange: (value: string) => void;
  onToChange: (value: string) => void;
}

export function DateRangePicker({
  from,
  to,
  onFromChange,
  onToChange,
}: DateRangePickerProps) {
  return (
    <div className="flex items-center gap-2">
      <label className="text-sm text-muted-foreground">From</label>
      <input
        type="date"
        value={from}
        onChange={(e) => onFromChange(e.target.value)}
        className="rounded-md border border-border bg-background px-3 py-1.5 text-sm"
      />
      <label className="text-sm text-muted-foreground">To</label>
      <input
        type="date"
        value={to}
        onChange={(e) => onToChange(e.target.value)}
        className="rounded-md border border-border bg-background px-3 py-1.5 text-sm"
      />
    </div>
  );
}
