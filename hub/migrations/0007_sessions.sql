-- Web interface logins: the SHA-256 of each session token, never the token itself.
CREATE TABLE sessions (
    token_hash BLOB PRIMARY KEY,
    expires_at INTEGER NOT NULL
);
