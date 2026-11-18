-- A command run from the new image before it is released (migrations), and a
-- schedule that makes a service a cron job instead of an always-on container.
ALTER TABLE services
    ADD COLUMN pre_deploy_command TEXT,
    ADD COLUMN cron_schedule      TEXT;

-- One-off and scheduled runs of a service's current image.
CREATE TABLE runs (
    id          UUID        PRIMARY KEY,
    service_id  UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    -- NULL runs the image's own command
    command     TEXT,
    trigger     TEXT        NOT NULL CHECK (trigger IN ('schedule', 'manual')),
    status      TEXT        NOT NULL DEFAULT 'running'
                            CHECK (status IN ('running', 'succeeded', 'failed')),
    exit_code   BIGINT,
    output      TEXT        NOT NULL DEFAULT '',
    error       TEXT,
    started_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX runs_by_service ON runs (service_id, started_at DESC);

-- Deploy jobs run side by side; jobs sharing a lock key (one service) run in order.
ALTER TABLE jobs ADD COLUMN lock_key TEXT;
UPDATE jobs SET lock_key = COALESCE(
    payload ->> 'service_id',
    payload ->> 'project_id',
    (SELECT d.service_id::text FROM deployments d WHERE d.id::text = jobs.payload ->> 'deployment_id')
);

-- Where failure (and optionally success) messages go.
CREATE TABLE notification_channels (
    id             UUID        PRIMARY KEY,
    name           TEXT        NOT NULL UNIQUE,
    kind           TEXT        NOT NULL CHECK (kind IN ('slack', 'discord', 'webhook')),
    url_enc        TEXT        NOT NULL,
    notify_success BOOLEAN     NOT NULL DEFAULT false,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
