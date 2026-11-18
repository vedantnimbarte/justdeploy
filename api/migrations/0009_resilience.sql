-- Email notifications. The SMTP server lives in settings (key 'smtp', JSON with
-- the password encrypted); an email channel's url_enc holds its addresses.
ALTER TABLE notification_channels DROP CONSTRAINT notification_channels_kind_check;
ALTER TABLE notification_channels
    ADD CONSTRAINT notification_channels_kind_check CHECK (kind IN ('slack', 'discord', 'webhook', 'email'));

-- Whether a custom domain last resolved to this cluster; NULL until checked.
ALTER TABLE domains ADD COLUMN dns_ok BOOLEAN;

-- One-time codes that stand in for an authenticator, stored hashed.
CREATE TABLE recovery_codes (
    user_id   UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    code_hash TEXT        NOT NULL,
    used_at   TIMESTAMPTZ,
    PRIMARY KEY (user_id, code_hash)
);

-- A Postgres read replica streams from the database it names.
ALTER TABLE services ADD COLUMN replica_of UUID REFERENCES services (id) ON DELETE SET NULL;

-- Copying one database's data into another: a new copy, or replacing data.
CREATE TABLE data_copies (
    id          UUID        PRIMARY KEY,
    source_id   UUID        REFERENCES services (id) ON DELETE SET NULL,
    target_id   UUID        NOT NULL REFERENCES services (id) ON DELETE CASCADE,
    status      TEXT        NOT NULL DEFAULT 'pending'
                            CHECK (status IN ('pending', 'running', 'succeeded', 'failed')),
    error       TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ
);
CREATE INDEX data_copies_target ON data_copies (target_id, created_at DESC);
