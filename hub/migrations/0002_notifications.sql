CREATE TABLE notifications (
    at INTEGER NOT NULL,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    -- What produced it, e.g. a rule name or a device id.
    source TEXT NOT NULL
);
CREATE INDEX notifications_by_time ON notifications (at);
