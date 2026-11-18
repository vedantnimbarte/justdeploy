-- Usage history: the 30-second samples rolled up into hourly averages, kept 90 days.
CREATE TABLE usage_hourly (
    service_id   UUID             NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    hour         TIMESTAMPTZ      NOT NULL,
    cpu_percent  DOUBLE PRECISION NOT NULL,
    memory_bytes BIGINT           NOT NULL,
    -- its volumes, measured at the end of the hour
    disk_bytes   BIGINT           NOT NULL DEFAULT 0,
    net_rx_bytes BIGINT           NOT NULL DEFAULT 0,
    net_tx_bytes BIGINT           NOT NULL DEFAULT 0,
    PRIMARY KEY (service_id, hour)
);

-- Every production line a service prints, kept JD_LOG_DAYS (7) and searchable.
CREATE EXTENSION IF NOT EXISTS pg_trgm;
CREATE TABLE log_lines (
    id         BIGSERIAL   PRIMARY KEY,
    service_id UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    ts         TIMESTAMPTZ NOT NULL,
    line       TEXT        NOT NULL
);
CREATE INDEX log_lines_by_service ON log_lines (service_id, id DESC);
CREATE INDEX log_lines_by_time ON log_lines (ts);
CREATE INDEX log_lines_text ON log_lines USING gin (line gin_trgm_ops);

-- One check a minute per public service, kept 90 days.
CREATE TABLE uptime_checks (
    service_id UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    ts         TIMESTAMPTZ NOT NULL,
    up         BOOLEAN     NOT NULL,
    status     INT,
    latency_ms INT,
    error      TEXT,
    PRIMARY KEY (service_id, ts)
);

-- A public status page per project, listing the services picked for it.
ALTER TABLE projects
    ADD COLUMN status_page   BOOLEAN NOT NULL DEFAULT false,
    ADD COLUMN status_domain TEXT UNIQUE;

ALTER TABLE services
    ADD COLUMN on_status_page      BOOLEAN NOT NULL DEFAULT false,
    -- Postgres point-in-time recovery: where the write-ahead log goes
    ADD COLUMN pitr_destination_id UUID REFERENCES backup_destinations (id) ON DELETE SET NULL,
    ADD COLUMN pitr_base_backup_at TIMESTAMPTZ,
    -- a database being made from another's history, filled before it first starts
    ADD COLUMN restore_from        UUID REFERENCES services (id) ON DELETE SET NULL,
    ADD COLUMN restore_target      TIMESTAMPTZ;
