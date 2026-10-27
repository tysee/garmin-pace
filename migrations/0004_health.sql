-- migrations/0004_health.sql stores atomic daily health snapshots and paged samples.
-- The primary keys support bounded calendar-range and per-metric sample reads.
CREATE TABLE health_days (
    date TEXT PRIMARY KEY NOT NULL CHECK (length(date) = 10),
    synced_at TEXT NOT NULL,
    document TEXT NOT NULL CHECK (length(document) <= 262144)
) STRICT;
CREATE TABLE health_samples (
    date TEXT NOT NULL REFERENCES health_days(date) ON DELETE CASCADE,
    metric TEXT NOT NULL,
    sequence INTEGER NOT NULL CHECK (sequence >= 0),
    timestamp TEXT NOT NULL,
    value REAL,
    unit TEXT NOT NULL,
    PRIMARY KEY (date, metric, sequence)
) STRICT, WITHOUT ROWID;
