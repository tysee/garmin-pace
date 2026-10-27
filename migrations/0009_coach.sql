-- migrations/0009_coach.sql stores bounded Coach calendar snapshots and draft reasoning.
-- Coach imports atomically replace a window and its workout library sources.
CREATE TABLE coach_snapshots (
    start_date TEXT NOT NULL,
    end_date TEXT NOT NULL,
    document TEXT NOT NULL CHECK(json_valid(document)),
    PRIMARY KEY(start_date, end_date)
) STRICT;
ALTER TABLE workout_drafts ADD COLUMN explanation TEXT;
