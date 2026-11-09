-- Ultra-Review: one row per review, kept whole as JSON (a few dozen findings
-- at most). `host` is the agent's id for the session the review belongs to.
CREATE TABLE ultra_reviews (
    id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL,
    host TEXT NOT NULL,
    status TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX ultra_reviews_by_host ON ultra_reviews(host, created_at);
