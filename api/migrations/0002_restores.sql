-- The outcome of the most recent restore from each backup.
ALTER TABLE backups
    ADD COLUMN restore_status TEXT CHECK (restore_status IN ('queued', 'running', 'succeeded', 'failed')),
    ADD COLUMN restore_error  TEXT,
    ADD COLUMN restored_at    TIMESTAMPTZ;
