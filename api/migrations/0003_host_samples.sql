-- Whole-server resources, sampled every 30 seconds and kept for 24 hours.
CREATE TABLE host_samples (
    ts             TIMESTAMPTZ      PRIMARY KEY,
    cpu_percent    DOUBLE PRECISION NOT NULL,
    cpu_cores      INT              NOT NULL,
    memory_used    BIGINT           NOT NULL,
    memory_total   BIGINT           NOT NULL,
    disk_used      BIGINT           NOT NULL,
    disk_total     BIGINT           NOT NULL,
    -- Cumulative counters across physical interfaces; rates are derived on read.
    net_rx_bytes   BIGINT           NOT NULL,
    net_tx_bytes   BIGINT           NOT NULL,
    uptime_seconds BIGINT           NOT NULL
);
