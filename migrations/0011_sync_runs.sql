-- migrations/0011_sync_runs.sql journals full syncs so an interrupted run can continue and
-- Status can tell a complete import from a partial one. Window rows key resume; step rows
-- hold each step's honest result. Only the newest runs are kept (pruned by the adapter).
CREATE TABLE sync_runs (
    id INTEGER PRIMARY KEY,
    start_date TEXT NOT NULL,
    end_date TEXT NOT NULL,
    started_at TEXT NOT NULL,
    finished_at TEXT,
    state TEXT NOT NULL CHECK (state IN ('running', 'complete', 'partial', 'stopped'))
) STRICT;

CREATE TABLE sync_run_windows (
    run_id INTEGER NOT NULL REFERENCES sync_runs(id) ON DELETE CASCADE,
    step TEXT NOT NULL CHECK (step IN ('activities', 'strength', 'health', 'coach')),
    start_date TEXT NOT NULL,
    end_date TEXT NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('done', 'failed')),
    saved INTEGER NOT NULL CHECK (saved >= 0),
    reason TEXT,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (run_id, step, start_date, end_date)
) STRICT;

CREATE TABLE sync_run_steps (
    run_id INTEGER NOT NULL REFERENCES sync_runs(id) ON DELETE CASCADE,
    step TEXT NOT NULL CHECK (step IN ('activities', 'strength', 'health', 'coach')),
    state TEXT NOT NULL CHECK (state IN ('complete', 'partial', 'failed', 'skipped')),
    windows INTEGER NOT NULL CHECK (windows > 0),
    done INTEGER NOT NULL CHECK (done >= 0),
    reused INTEGER NOT NULL CHECK (reused >= 0),
    saved INTEGER NOT NULL CHECK (saved >= 0),
    reason TEXT,
    recorded_at TEXT NOT NULL,
    PRIMARY KEY (run_id, step)
) STRICT;
