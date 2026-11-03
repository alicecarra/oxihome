## Database

The hub keeps its state and history in SQLite, in the file set in `[storage] path` (default
config: `hub/oxihome.sqlite`, out of git, with its `-wal` and `-shm` files). The file and its tables are created
at start when missing; the schema is in `hub/migrations/` and is applied by `sqlx` on start. SQLite
is built into the binary (`sqlite-bundled`), so no system package is needed. Dependencies added
on 2026-10-12:

```sh
cargo add -p oxihome-hub sqlx@0.9 --no-default-features -F runtime-tokio,sqlite-bundled,migrate,macros
cargo add -p oxihome-hub serde_json@1
```

What it keeps:

- `nodes`: the last `Hello` of each node (firmware, entities), when it was last seen, uptime, RSSI
  and heartbeat interval. After a restart the nodes show as offline with all their channels
  instead of "not seen", until they talk again; a lower uptime in the first heartbeat still shows
  as a reboot.
- `channel_values`: the last value of each channel, with its confidence, shown right after a
  restart; the `Query State` sent on connecting updates it.
- `readings`: every value change (history), with a `number` column for `Bool` (0 or 1), `Int` and
  `Float`, for charts.
- `events`: every event (button, IR code). The last event of each channel and the recent codes
  of the Learn section on `/remotes` are read back from it at start.
- `rule_runs`: the last run of each rule, shown on `/rules`. Runs of rules no longer in
  `rules.toml` (removed or renamed) are dropped at start and whenever the configuration is applied.
- `notifications`: the toasts (node offline, rule failed, `notify` actions), for the dashboard's
  `events` block.

`readings`, `events` and `notifications` keep `[storage] retention_days` days (default 7), pruned at start and
then every hour. Writes go to a task of their own, in one transaction per batch, so the MQTT loop
never waits for the disk. Times are milliseconds since the Unix epoch. To look inside (with the
`sqlite` package):

```sh
sqlite3 hub/oxihome.sqlite "SELECT device_id, entity, channel, datetime(at / 1000, 'unixepoch', 'localtime'), value FROM readings ORDER BY at DESC LIMIT 20"
```

Deleting the file starts from scratch: the nodes rebuild the state as before, from their `Hello`
and `State`.
