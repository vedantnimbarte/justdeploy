-- The single admin account. The CHECK keeps it to one row.
CREATE TABLE admin (
    id            INT         PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    email         TEXT        NOT NULL,
    password_hash TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE sessions (
    token_hash TEXT        PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE projects (
    id         UUID        PRIMARY KEY,
    name       TEXT        NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE services (
    id                    UUID        PRIMARY KEY,
    project_id            UUID        NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    name                  TEXT        NOT NULL,
    kind                  TEXT        NOT NULL CHECK (kind IN ('app', 'image', 'database')),
    -- app: where the code comes from and how to build it
    repo_url              TEXT,
    branch                TEXT,
    root_dir              TEXT        NOT NULL DEFAULT '',
    git_token_enc         TEXT,
    build_type            TEXT        NOT NULL DEFAULT 'auto'
                                      CHECK (build_type IN ('auto', 'dockerfile', 'railpack')),
    dockerfile_path       TEXT        NOT NULL DEFAULT 'Dockerfile',
    -- image and database: what to run
    image                 TEXT,
    engine                TEXT        CHECK (engine IN ('postgres', 'mysql', 'mariadb', 'redis', 'mongodb')),
    -- runtime
    port                  INT,
    public                BOOLEAN     NOT NULL DEFAULT false,
    health_path           TEXT,
    cpu_millis            INT,
    memory_mb             INT,
    public_port           INT         UNIQUE,
    preview_enabled       BOOLEAN     NOT NULL DEFAULT false,
    webhook_secret        TEXT        NOT NULL,
    desired_state         TEXT        NOT NULL DEFAULT 'running'
                                      CHECK (desired_state IN ('running', 'stopped', 'deleting')),
    current_deployment_id UUID,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project_id, name)
);

CREATE TABLE env_vars (
    service_id UUID NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value_enc  TEXT NOT NULL,
    PRIMARY KEY (service_id, key)
);

CREATE TABLE domains (
    id         UUID        PRIMARY KEY,
    service_id UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    hostname   TEXT        NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE volumes (
    id         UUID NOT NULL PRIMARY KEY,
    service_id UUID NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    name       TEXT NOT NULL UNIQUE,
    mount_path TEXT NOT NULL,
    UNIQUE (service_id, mount_path)
);

CREATE TABLE deployments (
    id             UUID        PRIMARY KEY,
    service_id     UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    kind           TEXT        NOT NULL DEFAULT 'production' CHECK (kind IN ('production', 'preview')),
    pr_number      INT,
    -- previews from forks build from the fork, so the source is per deployment
    repo_url       TEXT,
    branch         TEXT,
    commit_sha     TEXT,
    commit_message TEXT,
    image_ref      TEXT,
    status         TEXT        NOT NULL DEFAULT 'queued'
                               CHECK (status IN ('queued', 'building', 'deploying', 'running',
                                                 'failed', 'superseded', 'removed')),
    trigger        TEXT        NOT NULL,
    error          TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at     TIMESTAMPTZ,
    finished_at    TIMESTAMPTZ
);
CREATE INDEX deployments_by_service ON deployments (service_id, created_at DESC);

ALTER TABLE services
    ADD CONSTRAINT services_current_deployment_fk
    FOREIGN KEY (current_deployment_id) REFERENCES deployments (id) ON DELETE SET NULL;

CREATE TABLE deployment_logs (
    id            BIGSERIAL   PRIMARY KEY,
    deployment_id UUID        NOT NULL REFERENCES deployments (id) ON DELETE CASCADE,
    line          TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX deployment_logs_by_deployment ON deployment_logs (deployment_id, id);

-- Durable work queue. `queue` separates builds and deploys from backups so a long
-- build never holds up a scheduled backup.
CREATE TABLE jobs (
    id         BIGSERIAL   PRIMARY KEY,
    queue      TEXT        NOT NULL,
    kind       TEXT        NOT NULL,
    payload    JSONB       NOT NULL,
    attempts   INT         NOT NULL DEFAULT 0,
    run_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_at  TIMESTAMPTZ,
    last_error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX jobs_ready ON jobs (queue, run_at) WHERE locked_at IS NULL;

CREATE TABLE backup_destinations (
    id         UUID        PRIMARY KEY,
    name       TEXT        NOT NULL UNIQUE,
    kind       TEXT        NOT NULL CHECK (kind IN ('s3', 'local')),
    config_enc TEXT        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE backup_schedules (
    service_id     UUID        PRIMARY KEY REFERENCES services (id) ON DELETE CASCADE,
    destination_id UUID        NOT NULL REFERENCES backup_destinations (id) ON DELETE CASCADE,
    cron           TEXT        NOT NULL,
    keep           INT         NOT NULL DEFAULT 7 CHECK (keep > 0),
    last_run_at    TIMESTAMPTZ
);

CREATE TABLE backups (
    id             UUID        PRIMARY KEY,
    service_id     UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    destination_id UUID        REFERENCES backup_destinations (id) ON DELETE SET NULL,
    object_key     TEXT        NOT NULL,
    status         TEXT        NOT NULL DEFAULT 'running'
                               CHECK (status IN ('running', 'succeeded', 'failed')),
    size_bytes     BIGINT,
    error          TEXT,
    trigger        TEXT        NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at    TIMESTAMPTZ
);
CREATE INDEX backups_by_service ON backups (service_id, created_at DESC);

CREATE TABLE metric_samples (
    service_id   UUID             NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    ts           TIMESTAMPTZ      NOT NULL,
    cpu_percent  DOUBLE PRECISION NOT NULL,
    memory_bytes BIGINT           NOT NULL,
    net_rx_bytes BIGINT           NOT NULL,
    net_tx_bytes BIGINT           NOT NULL,
    PRIMARY KEY (service_id, ts)
);

CREATE TABLE audit_log (
    id         BIGSERIAL   PRIMARY KEY,
    action     TEXT        NOT NULL,
    target     TEXT        NOT NULL,
    detail     TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
