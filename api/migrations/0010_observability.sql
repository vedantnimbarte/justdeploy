-- Requests and traces: OpenTelemetry spans from Traefik (one per request,
-- source 'edge') and from apps that send their own (source 'app'). Kept
-- `JD_LOG_DAYS` days, like log lines.
CREATE TABLE spans (
    id             BIGSERIAL PRIMARY KEY,
    trace_id       TEXT NOT NULL,
    span_id        TEXT NOT NULL,
    parent_span_id TEXT,
    deployment_id  UUID,
    service_id     UUID REFERENCES services(id) ON DELETE CASCADE,
    source         TEXT NOT NULL,
    name           TEXT NOT NULL,
    kind           SMALLINT NOT NULL,
    start_ts       TIMESTAMPTZ NOT NULL,
    duration_us    BIGINT NOT NULL,
    -- OpenTelemetry status: 0 unset, 1 ok, 2 error.
    status         SMALLINT NOT NULL,
    http_method    TEXT,
    http_host      TEXT,
    http_path      TEXT,
    http_status    INT,
    client_ip      TEXT,
    attrs          JSONB NOT NULL DEFAULT '{}'
);
CREATE INDEX spans_service_ts ON spans (service_id, start_ts DESC);
CREATE INDEX spans_trace ON spans (trace_id);
CREATE INDEX spans_ts ON spans (start_ts);
CREATE INDEX spans_requests ON spans (service_id, start_ts DESC) WHERE source = 'edge';

-- Structured logs: the level a line was printed at (0 debug, 1 info, 2 warn,
-- 3 error), the trace it belongs to, and its fields when it was JSON.
ALTER TABLE log_lines
    ADD COLUMN deployment_id UUID,
    ADD COLUMN level SMALLINT NOT NULL DEFAULT 1,
    ADD COLUMN trace_id TEXT,
    ADD COLUMN fields JSONB;
CREATE INDEX log_lines_trace ON log_lines (trace_id) WHERE trace_id IS NOT NULL;
