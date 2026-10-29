-- migrations/0012_coach_reschedules.sql journals explicit moves of one scheduled Coach session.
-- A row is written before Garmin is contacted; at most one move per schedule ID stays unresolved.
CREATE TABLE coach_reschedules (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    schedule_id INTEGER NOT NULL CHECK (schedule_id > 0),
    workout_id INTEGER NOT NULL CHECK (workout_id > 0),
    from_date TEXT NOT NULL,
    to_date TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('moving', 'moved', 'not_applied', 'missing')),
    requested_at TEXT NOT NULL,
    mcp_request TEXT
) STRICT;
CREATE UNIQUE INDEX coach_reschedule_pending ON coach_reschedules(schedule_id) WHERE state = 'moving';
CREATE INDEX coach_reschedule_dates ON coach_reschedules(from_date, to_date);
