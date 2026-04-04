import {
  Table,
  TableHeader,
  TableBody,
  TableRow,
  TableHead,
  TableCell,
} from "../ui/table";
import { formatUsd, formatNumber } from "../../lib/utils";

interface CostRow {
  label: string;
  cost_usd: number;
  total_tokens: number;
  run_count: number;
}

interface CostTableProps {
  title: string;
  labelHeader: string;
  rows: CostRow[];
}

export function CostTable({ title, labelHeader, rows }: CostTableProps) {
  return (
    <div>
      <h3 className="text-lg font-semibold mb-3">{title}</h3>
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>{labelHeader}</TableHead>
            <TableHead className="text-right">Cost (USD)</TableHead>
            <TableHead className="text-right">Total Tokens</TableHead>
            <TableHead className="text-right">Runs</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.map((row) => (
            <TableRow key={row.label}>
              <TableCell className="font-medium">{row.label}</TableCell>
              <TableCell className="text-right">
                {formatUsd(row.cost_usd)}
              </TableCell>
              <TableCell className="text-right">
                {formatNumber(row.total_tokens)}
              </TableCell>
              <TableCell className="text-right">
                {formatNumber(row.run_count)}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
