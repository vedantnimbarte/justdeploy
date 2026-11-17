import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Navigate, Route, Routes } from "react-router";
import { api } from "./api";
import Layout from "./Layout";
import AnalyticsPage from "./pages/Analytics";
import AuthPage from "./pages/Auth";
import NewServicePage from "./pages/NewService";
import ProjectPage from "./pages/Project";
import ProjectsPage from "./pages/Projects";
import ServicePage from "./pages/Service";
import SettingsPage from "./pages/Settings";
import TemplatesPage from "./pages/Templates";
import "./styles.css";
import { ToastProvider } from "./ui";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 3_000, retry: 1, refetchOnWindowFocus: false },
  },
});

/** First run shows setup, a signed-out visit shows login, otherwise the app. */
function AuthGate() {
  const status = useQuery({
    queryKey: ["auth-status"],
    queryFn: () => api.get<{ setup_required: boolean; authenticated: boolean }>("/auth/status"),
  });
  if (status.isPending) return null;
  if (status.isError) {
    return <p className="p-8">Cannot reach the justdeploy API. Is it running?</p>;
  }
  if (status.data.setup_required) return <AuthPage mode="setup" />;
  if (!status.data.authenticated) return <AuthPage mode="login" />;
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route path="/projects" element={<ProjectsPage />} />
        <Route path="/projects/:id" element={<ProjectPage />} />
        <Route path="/projects/:id/new" element={<NewServicePage />} />
        <Route path="/services/:id" element={<ServicePage />} />
        <Route path="/analytics" element={<AnalyticsPage />} />
        <Route path="/templates" element={<TemplatesPage />} />
        <Route path="/settings" element={<SettingsPage />} />
      </Route>
      <Route path="*" element={<Navigate to="/projects" replace />} />
    </Routes>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <ToastProvider>
        <BrowserRouter>
          <AuthGate />
        </BrowserRouter>
      </ToastProvider>
    </QueryClientProvider>
  </StrictMode>,
);
