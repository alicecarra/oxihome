-- Over-the-air updates, kept for good: they are few and worth remembering.
CREATE TABLE updates (
    device_id TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    finished_at INTEGER NOT NULL,
    from_version TEXT NOT NULL,
    to_version TEXT NOT NULL,
    -- updated, rolled_back, failed or no_answer.
    outcome TEXT NOT NULL,
    detail TEXT
);
CREATE INDEX updates_by_node ON updates (device_id, finished_at);

-- A node page lists the notifications about that node.
CREATE INDEX notifications_by_source ON notifications (source, at);
