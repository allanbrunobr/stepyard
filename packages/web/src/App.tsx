import { Routes, Route, Navigate } from 'react-router-dom';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { Layout } from '@/components/layout/Layout';
import { OverviewPage } from '@/pages/Overview';
import { WorkflowLogPage } from './pages/Workflows/WorkflowLogPage';
import { WorkflowDetailPage } from './pages/WorkflowDetail/WorkflowDetailPage';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      refetchOnWindowFocus: false,
      retry: 1,
    },
  },
});

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <Layout>
        <Routes>
          <Route path="/" element={<OverviewPage />} />
          <Route path="/workflows" element={<WorkflowLogPage />} />
          <Route path="/workflows/:runId" element={<WorkflowDetailPage />} />
          <Route path="/developers" element={<Navigate to="/" replace />} />
          <Route path="/costs" element={<Navigate to="/" replace />} />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </Layout>
    </QueryClientProvider>
  );
}
