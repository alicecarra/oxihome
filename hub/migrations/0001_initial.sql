-- Times are milliseconds since the Unix epoch.

CREATE TABLE nodes (
    device_id TEXT PRIMARY KEY NOT NULL,
    firmware_version TEXT NOT NULL,
    -- The entities of the last `Hello`, as protocol JSON.
    entities TEXT NOT NULL,
    last_seen INTEGER NOT NULL,
    uptime_seconds INTEGER,
    rssi INTEGER,
    heartbeat_interval_seconds INTEGER NOT NULL
);

CREATE TABLE channel_values (
    device_id TEXT NOT NULL,
    entity INTEGER NOT NULL,
    channel INTEGER NOT NULL,
    value TEXT NOT NULL,
    confidence TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (device_id, entity, channel)
) WITHOUT ROWID;

CREATE TABLE readings (
    device_id TEXT NOT NULL,
    entity INTEGER NOT NULL,
    channel INTEGER NOT NULL,
    at INTEGER NOT NULL,
    value TEXT NOT NULL,
    -- Bool, Int and Float values, for charts.
    number REAL,
    confidence TEXT NOT NULL
);
CREATE INDEX readings_by_channel ON readings (device_id, entity, channel, at);
CREATE INDEX readings_by_time ON readings (at);

CREATE TABLE events (
    device_id TEXT NOT NULL,
    entity INTEGER NOT NULL,
    channel INTEGER NOT NULL,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    data TEXT
);
CREATE INDEX events_by_channel ON events (device_id, entity, channel, at);
CREATE INDEX events_by_time ON events (at);

CREATE TABLE rule_runs (
    name TEXT PRIMARY KEY NOT NULL,
    at INTEGER NOT NULL,
    -- JSON array of texts.
    errors TEXT NOT NULL
);
