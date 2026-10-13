-- Every rule run for the timeline; `rule_runs` keeps only the last one of each rule.
CREATE TABLE rule_executions (
    name TEXT NOT NULL,
    at INTEGER NOT NULL,
    -- JSON array of texts.
    errors TEXT NOT NULL
);
CREATE INDEX rule_executions_by_time ON rule_executions (at);
INSERT INTO rule_executions (name, at, errors) SELECT name, at, errors FROM rule_runs;

-- Commands sent from the web interface or by rules.
CREATE TABLE commands (
    device_id TEXT NOT NULL,
    entity INTEGER NOT NULL,
    channel INTEGER NOT NULL,
    at INTEGER NOT NULL,
    -- set or invoke.
    operation TEXT NOT NULL,
    -- Protocol JSON; NULL for an invoke without a value.
    value TEXT,
    -- The rule that sent it; NULL when sent from the web interface.
    rule TEXT,
    -- Why the node refused it or did not answer.
    error TEXT
);
CREATE INDEX commands_by_time ON commands (at);
CREATE INDEX commands_by_node ON commands (device_id, at);
