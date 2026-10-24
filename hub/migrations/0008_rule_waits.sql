-- Rules waiting out their `for`, so a restart does not lose them.
CREATE TABLE rule_waits (
    rule TEXT NOT NULL,
    device_id TEXT NOT NULL,
    -- The rule's `when` as the hub prints it; a different one after the restart drops the wait.
    trigger TEXT NOT NULL,
    deadline INTEGER NOT NULL,
    -- JSON of the names and value the rule's texts fill in.
    subject TEXT NOT NULL,
    PRIMARY KEY (rule, device_id)
) WITHOUT ROWID;
