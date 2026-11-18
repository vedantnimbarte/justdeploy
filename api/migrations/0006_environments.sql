-- Environments: a project row per environment. The first (production) is the
-- base; staging and the like point at it and are named `<base>-<environment>`,
-- so their networks, hostnames and volumes never collide with production's.
ALTER TABLE projects
    ADD COLUMN environment TEXT NOT NULL DEFAULT 'production',
    ADD COLUMN base_id     UUID REFERENCES projects (id) ON DELETE SET NULL;
CREATE UNIQUE INDEX projects_one_per_environment ON projects (base_id, environment)
    WHERE base_id IS NOT NULL;
