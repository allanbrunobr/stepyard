import { Routes, Route } from 'react-router-dom';
import { Layout } from '@/components/layout/Layout';
import { OverviewPage } from '@/pages/Overview';

function PlaceholderPage({ title }: { title: string }) {
  return (
    <div className="flex items-center justify-center h-64">
      <p className="text-lg text-[hsl(var(--muted-foreground))]">{title} — Coming Soon</p>
    </div>
  );
}

export default function App() {
  return (
    <Layout>
      <Routes>
        <Route path="/" element={<OverviewPage />} />
        <Route path="/workflows" element={<PlaceholderPage title="Workflow Log" />} />
        <Route path="/developers" element={<PlaceholderPage title="Developer Activity" />} />
        <Route path="/costs" element={<PlaceholderPage title="Cost Tracking" />} />
      </Routes>
    </Layout>
  );
}
