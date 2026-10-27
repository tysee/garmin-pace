-- migrations/0007_mcp_requests.sql journals the user instruction a model quoted for a Garmin write.
-- NULL means the write was started from the CLI or menu; MCP records the request before calling Garmin.
ALTER TABLE workout_drafts ADD COLUMN mcp_request TEXT;
ALTER TABLE benchmark_drafts ADD COLUMN mcp_request TEXT;
ALTER TABLE workout_deliveries ADD COLUMN mcp_request TEXT;
