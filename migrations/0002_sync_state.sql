-- migrations/0002_sync_state.sql records the scope and time of a successful import.
-- The sync use case commits this singleton together with its activity upserts.
CREATE TABLE sync_state (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    completed_at TEXT NOT NULL,
    requested_limit INTEGER NOT NULL CHECK (requested_limit > 0),
    received_count INTEGER NOT NULL CHECK (received_count >= 0)
) STRICT;
