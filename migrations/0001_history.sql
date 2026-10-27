-- migrations/0001_history.sql establishes the local activity summary contract.
-- It creates the history table and index used by bounded chronological queries.
CREATE TABLE activities (
    garmin_id TEXT PRIMARY KEY NOT NULL CHECK (length(garmin_id) > 0),
    name TEXT NOT NULL,
    sport TEXT NOT NULL,
    started_at TEXT NOT NULL,
    duration_seconds REAL NOT NULL CHECK (duration_seconds >= 0),
    distance_meters REAL CHECK (distance_meters >= 0)
) STRICT;

CREATE INDEX activities_chronological ON activities(started_at DESC, garmin_id DESC);
