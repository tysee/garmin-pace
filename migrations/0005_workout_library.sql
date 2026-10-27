-- migrations/0005_workout_library.sql adds profile-local workout snapshots and model drafts.
-- Remote writes and deliveries are reserved durably; migration 3's ledger remains historical.
CREATE TABLE workout_library (
    id INTEGER PRIMARY KEY CHECK (id > 0),
    name TEXT NOT NULL, sport TEXT NOT NULL, revision TEXT NOT NULL,
    synced_at TEXT NOT NULL, document TEXT NOT NULL CHECK(json_valid(document))
) STRICT;
CREATE INDEX workout_library_name ON workout_library(name COLLATE NOCASE, id);
CREATE TABLE workout_drafts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision > 0),
    definition TEXT NOT NULL CHECK(json_valid(definition)),
    source_id INTEGER, source_document TEXT CHECK(source_document IS NULL OR json_valid(source_document)),
    state TEXT NOT NULL DEFAULT 'draft' CHECK(state IN ('draft','saving','saved','verified','discarded')),
    remote_id INTEGER CHECK(remote_id IS NULL OR remote_id > 0),
    CHECK((source_id IS NULL) = (source_document IS NULL))
) STRICT;
CREATE UNIQUE INDEX workout_pending_source ON workout_drafts(source_id) WHERE state IN ('saving','saved');
CREATE INDEX workout_draft_state ON workout_drafts(state, id);
CREATE TABLE workout_deliveries (
    workout_id INTEGER NOT NULL CHECK(workout_id > 0),
    device_id INTEGER NOT NULL CHECK(device_id > 0),
    revision TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('sending','queued')),
    PRIMARY KEY(workout_id, device_id, revision)
) STRICT;
