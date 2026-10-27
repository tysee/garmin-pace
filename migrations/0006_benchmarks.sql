-- migrations/0006_benchmarks.sql stores benchmark sources, observed sets and model estimates.
-- Estimates remain local until reviewed; pending writes cannot be retried automatically.
CREATE TABLE benchmark_catalog (key TEXT PRIMARY KEY, document TEXT NOT NULL CHECK(json_valid(document))) STRICT;
CREATE TABLE strength_sessions (
    activity_id INTEGER PRIMARY KEY CHECK(activity_id>0), date TEXT NOT NULL,
    document TEXT NOT NULL CHECK(json_valid(document))
) STRICT;
CREATE INDEX strength_session_date ON strength_sessions(date, activity_id);
CREATE TABLE strength_imports (
    start TEXT NOT NULL, end TEXT NOT NULL, synced_at TEXT NOT NULL,
    PRIMARY KEY(start,end)
) STRICT;
CREATE TABLE benchmark_drafts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    revision INTEGER NOT NULL DEFAULT 1 CHECK(revision>0),
    key TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'draft' CHECK(state IN ('draft','saving','saved','verified','discarded')),
    proposal TEXT NOT NULL CHECK(json_valid(proposal)), context TEXT NOT NULL CHECK(json_valid(context))
) STRICT;
CREATE UNIQUE INDEX benchmark_pending_key ON benchmark_drafts(key) WHERE state IN ('saving','saved');
CREATE INDEX benchmark_draft_state ON benchmark_drafts(state,id);
