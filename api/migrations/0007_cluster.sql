-- Docker Swarm: how many copies of a service run, where, and when to add more.
ALTER TABLE services
    -- copies running now; autoscaling moves it between autoscale_min and autoscale_max
    ADD COLUMN replicas      INT  NOT NULL DEFAULT 1 CHECK (replicas >= 1),
    ADD COLUMN autoscale_min INT  CHECK (autoscale_min >= 1),
    ADD COLUMN autoscale_max INT  CHECK (autoscale_max >= 1),
    -- the average CPU per copy to aim for, in percent of its limit (or of one core)
    ADD COLUMN autoscale_cpu INT  NOT NULL DEFAULT 70 CHECK (autoscale_cpu BETWEEN 10 AND 100),
    -- the server it is pinned to (a Swarm node id); always set for services with a volume
    ADD COLUMN node_id       TEXT,
    -- `key=value`: run only on servers carrying that label
    ADD COLUMN node_label    TEXT;

-- Server samples are per server now. They are kept a day, so starting over loses little.
DELETE FROM host_samples;
ALTER TABLE host_samples
    DROP CONSTRAINT host_samples_pkey,
    ADD COLUMN node_id TEXT NOT NULL,
    ADD PRIMARY KEY (node_id, ts);
