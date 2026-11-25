import { Link } from "react-router";
import { type ServiceDetail } from "../../api";
import { SectionTitle } from "../../ui";
import { LogPanel } from "./LogPanel";

export default function Logs({ service }: { service: ServiceDetail }) {
  return (
    <>
      <SectionTitle>Runtime output</SectionTitle>
      <p className="mb-5 max-w-3xl">
        What the running container prints, followed live. Build output lives under Deployments; to search kept lines by text or level, or
        see the requests behind them, open <Link to={`/services/${service.id}?tab=observability`}>Observability</Link>.
      </p>
      {/* Remount after a redeploy so the stream follows the new container. */}
      <LogPanel
        key={service.current_deployment_id ?? "none"}
        url={`/api/v1/services/${service.id}/logs?tail=500`}
        title={`${service.name} output`}
        height="h-[36rem]"
      />
    </>
  );
}
