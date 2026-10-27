-- migrations/0010_standalone_workouts.sql indexes the standalone library view separately.
-- Coach-owned definitions remain available through the Coach schedule and all-workout API.
CREATE INDEX workout_library_standalone ON workout_library(name COLLATE NOCASE, id)
WHERE json_extract(document, '$.coach_plan_id') IS NULL;
