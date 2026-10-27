-- migrations/access/0002_garmin_writes.sql adds the per-user grant for model-initiated Garmin writes.
-- Existing users stay read-only; the local administrator enables it explicitly.
ALTER TABLE users ADD COLUMN garmin_writes INTEGER NOT NULL DEFAULT 0 CHECK(garmin_writes IN (0,1));
