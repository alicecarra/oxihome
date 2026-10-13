-- The protocol version of the node's last `Hello`: the hub also reads the previous version.
ALTER TABLE nodes ADD COLUMN protocol_version INTEGER NOT NULL DEFAULT 1;
