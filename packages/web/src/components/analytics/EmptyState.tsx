interface EmptyStateProps {
  message?: string;
}

export function EmptyState({ message = "No data found for the selected date range." }: EmptyStateProps) {
  return (
    <div className="flex items-center justify-center py-12">
      <div className="text-muted-foreground text-sm">{message}</div>
    </div>
  );
}
