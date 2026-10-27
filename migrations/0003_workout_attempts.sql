-- migrations/0003_workout_attempts.sql journals one-shot Garmin workout writes.
-- Names reserve publication attempts; pending states retain ambiguous outcomes.
CREATE TABLE workout_attempts (
    name TEXT PRIMARY KEY NOT NULL,
    workout_id INTEGER UNIQUE,
    device_id INTEGER,
    state TEXT NOT NULL CHECK (state IN ('publish_pending','saved','verified','send_pending','queued')),
    CHECK (workout_id IS NULL OR workout_id > 0),
    CHECK (device_id IS NULL OR device_id > 0)
) STRICT;
