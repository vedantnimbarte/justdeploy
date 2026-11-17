import { lazy, Suspense } from "react";
import { Band, PageHeader } from "../ui";

// Charts are the heaviest code in the dashboard; load them only on this page.
const SystemOverview = lazy(() => import("./SystemOverview"));

export default function AnalyticsPage() {
  return (
    <>
      <PageHeader
        title="Analytics"
        lead="How the server and everything on it is doing: resources, activity, disk usage, and anything that needs attention."
      />
      <Band tone="white">
        <Suspense fallback={<p>Loading…</p>}>
          <SystemOverview />
        </Suspense>
      </Band>
    </>
  );
}
