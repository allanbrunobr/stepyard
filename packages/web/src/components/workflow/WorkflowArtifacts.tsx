import { Download, Paperclip } from 'lucide-react';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import type { WorkflowArtifact } from '@/types';
import { formatTimestamp } from '@/lib/format';

interface WorkflowArtifactsProps {
  artifacts: WorkflowArtifact[];
  isLoading: boolean;
  error?: Error | null;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const kb = bytes / 1024;
  if (kb < 1024) return `${kb.toFixed(1)} KB`;
  const mb = kb / 1024;
  return `${mb.toFixed(1)} MB`;
}

export function WorkflowArtifacts({ artifacts, isLoading, error }: WorkflowArtifactsProps) {
  return (
    <Card className="mb-6">
      <CardHeader>
        <CardTitle className="text-lg flex items-center gap-2">
          <Paperclip className="h-4 w-4" />
          Artifacts
        </CardTitle>
      </CardHeader>
      <CardContent>
        {isLoading && <p className="text-muted-foreground text-sm py-2">Loading artifacts...</p>}

        {error && (
          <div className="rounded-md bg-destructive/10 text-destructive p-3 text-sm">
            Failed to load artifacts: {error.message}
          </div>
        )}

        {!isLoading && !error && artifacts.length === 0 && (
          <p className="text-muted-foreground text-sm py-2">No artifacts uploaded for this run.</p>
        )}

        {!isLoading && !error && artifacts.length > 0 && (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Type</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>Uploaded</TableHead>
                <TableHead className="w-12">Download</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {artifacts.map((artifact) => (
                <TableRow key={artifact.artifact_id}>
                  <TableCell className="font-medium break-all">{artifact.name}</TableCell>
                  <TableCell>{artifact.content_type ?? 'application/octet-stream'}</TableCell>
                  <TableCell>{formatBytes(artifact.size_bytes)}</TableCell>
                  <TableCell>{formatTimestamp(artifact.created_at)}</TableCell>
                  <TableCell>
                    <a
                      className="inline-flex h-9 w-9 items-center justify-center rounded-md border border-input bg-background hover:bg-accent hover:text-accent-foreground"
                      href={artifact.download_url}
                      aria-label={`Download ${artifact.name}`}
                    >
                      <Download className="h-4 w-4" />
                    </a>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  );
}
