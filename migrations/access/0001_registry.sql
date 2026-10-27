-- migrations/access/0001_registry.sql isolates MCP identities from Garmin history.
-- It stores canonical profile resources and revocable hashes, never bearer secrets.
CREATE TABLE users (
    name TEXT PRIMARY KEY NOT NULL,
    database_path TEXT NOT NULL UNIQUE,
    session_path TEXT NOT NULL UNIQUE,
    enabled INTEGER NOT NULL CHECK(enabled IN (0,1))
) STRICT;
CREATE TABLE access_keys (
    id TEXT PRIMARY KEY NOT NULL,
    user_name TEXT NOT NULL REFERENCES users(name),
    digest BLOB NOT NULL UNIQUE CHECK(length(digest)=32),
    expires_at INTEGER NOT NULL,
    revoked INTEGER NOT NULL CHECK(revoked IN (0,1))
) STRICT;
CREATE INDEX access_keys_user ON access_keys(user_name);
