-- migrations/0008_activity_search.sql indexes sport-scoped chronological history pages.
-- UTC range queries reuse this index or the existing all-sport chronological index.
CREATE INDEX activities_sport_chronological ON activities(sport, started_at DESC, garmin_id DESC);
