import { BrowserRouter, Routes, Route } from "react-router-dom";
import { AppLayout } from "./components/layout";
import { DevelopersPage } from "./pages/Developers";
import { CostsPage } from "./pages/Costs";

function App() {
  return (
    <BrowserRouter>
      <AppLayout>
        <Routes>
          <Route path="/" element={<div>Overview (wt2)</div>} />
          <Route path="/workflows" element={<div>Workflows (wt3)</div>} />
          {/* Route registrations below — append only */}
          <Route path="/developers" element={<DevelopersPage />} />
          <Route path="/costs" element={<CostsPage />} />
        </Routes>
      </AppLayout>
    </BrowserRouter>
  );
}

export default App;
