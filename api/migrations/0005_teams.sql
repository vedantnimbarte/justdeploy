-- People. The single admin becomes the first owner.
-- Roles, each including the one below: viewer (read-only, no secrets), member
-- (deploy and change services), admin (delete, settings), owner (people).
CREATE TABLE users (
    id               UUID        PRIMARY KEY,
    email            TEXT        NOT NULL UNIQUE,
    password_hash    TEXT        NOT NULL,
    role             TEXT        NOT NULL CHECK (role IN ('owner', 'admin', 'member', 'viewer')),
    -- set once two-factor sign-in is on; the pending one while it is being set up
    totp_secret_enc  TEXT,
    totp_pending_enc TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
INSERT INTO users (id, email, password_hash, role, created_at)
    SELECT gen_random_uuid(), email, password_hash, 'owner', created_at FROM admin;
DROP TABLE admin;

ALTER TABLE sessions ADD COLUMN user_id UUID REFERENCES users (id) ON DELETE CASCADE;
UPDATE sessions SET user_id = (SELECT id FROM users LIMIT 1);
DELETE FROM sessions WHERE user_id IS NULL;
ALTER TABLE sessions ALTER COLUMN user_id SET NOT NULL;

-- One-time links that create an account.
CREATE TABLE invites (
    id         UUID        PRIMARY KEY,
    token_hash TEXT        NOT NULL UNIQUE,
    email      TEXT        NOT NULL,
    role       TEXT        NOT NULL CHECK (role IN ('admin', 'member', 'viewer')),
    invited_by UUID        REFERENCES users (id) ON DELETE SET NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Bearer tokens for the API and the CLI. They act as their owner.
CREATE TABLE api_tokens (
    id           UUID        PRIMARY KEY,
    user_id      UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    name         TEXT        NOT NULL,
    token_hash   TEXT        NOT NULL UNIQUE,
    last_used_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Who did it: an email, or NULL for justdeploy itself.
ALTER TABLE audit_log ADD COLUMN actor TEXT;
CREATE INDEX audit_log_by_time ON audit_log (id DESC);

-- Variables every service in a project gets; a service's own value wins.
CREATE TABLE shared_env_vars (
    project_id UUID NOT NULL REFERENCES projects (id) ON DELETE CASCADE,
    key        TEXT NOT NULL,
    value_enc  TEXT NOT NULL,
    PRIMARY KEY (project_id, key)
);

-- The GitHub App this install created through GitHub's manifest flow.
CREATE TABLE github_app (
    id                 INT         PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    app_id             BIGINT      NOT NULL,
    slug               TEXT        NOT NULL,
    html_url           TEXT        NOT NULL,
    private_key_enc    TEXT        NOT NULL,
    webhook_secret_enc TEXT        NOT NULL,
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);
