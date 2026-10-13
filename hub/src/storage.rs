//! The hub's state and history in SQLite, so a restart loses almost nothing.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use jiff::{SignedDuration, Timestamp};
use oxihome_protocol::{Build, Confidence, EntityDescriptor, EventKind, Value};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqliteSynchronous};
use sqlx::{Row, Sqlite, Transaction};
use tokio::sync::{mpsc, oneshot};

use crate::config::rules::RuleRun;
use crate::node::events::EventRecord;
use crate::node::registry::StoredNode;
use crate::node::state::ChannelUpdate;

const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);
/// Received events read back at start, enough to fill the recent codes after skipping repeats.
const RECEIVED_EVENTS: i64 = 200;

pub enum Write {
    Node(StoredNode),
    Value(ChannelUpdate, Timestamp),
    Event(EventRecord, Timestamp),
    RuleRun(String, RuleRun),
    Notification(StoredNotification),
    Update(StoredUpdate),
    /// Answered once everything sent before it is written.
    Flush(oneshot::Sender<()>),
}

/// What the hub knew when it stopped.
#[derive(Default)]
pub struct Restored {
    pub nodes: Vec<StoredNode>,
    pub values: Vec<(String, u8, u8, Value, Confidence)>,
    /// The last event of each channel.
    pub last_events: Vec<(EventRecord, Timestamp)>,
    /// Newest first.
    pub received_events: Vec<(EventRecord, Timestamp)>,
    pub rule_runs: Vec<(String, RuleRun)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StoredNotification {
    pub title: String,
    pub message: String,
    pub source: String,
    pub at: Timestamp,
}

/// A finished over-the-air update.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredUpdate {
    pub device_id: String,
    pub started_at: Timestamp,
    pub finished_at: Timestamp,
    pub from_version: String,
    pub to_version: String,
    pub outcome: UpdateOutcome,
    /// Why it failed, as the node or the hub put it.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    Updated,
    /// The node came back with the old version.
    RolledBack,
    /// Refused or reported an error; the node kept its version.
    Failed,
    NoAnswer,
}

impl UpdateOutcome {
    fn key(self) -> &'static str {
        match self {
            Self::Updated => "updated",
            Self::RolledBack => "rolled_back",
            Self::Failed => "failed",
            Self::NoAnswer => "no_answer",
        }
    }

    fn from_key(key: &str) -> anyhow::Result<Self> {
        Ok(match key {
            "updated" => Self::Updated,
            "rolled_back" => Self::RolledBack,
            "failed" => Self::Failed,
            "no_answer" => Self::NoAnswer,
            other => anyhow::bail!("unknown update outcome {other}"),
        })
    }
}

/// Numeric readings of one channel, grouped in buckets of equal length.
#[derive(Debug, Default, PartialEq)]
pub struct History {
    /// The last value before the first bucket.
    pub before: Option<f64>,
    /// Bucket index from the start, average, minimum and maximum.
    pub buckets: Vec<(usize, f64, f64, f64)>,
}

pub struct Storage {
    pool: SqlitePool,
    writes: mpsc::UnboundedSender<Write>,
}

impl Storage {
    /// Creates the database when missing and keeps readings and events for `retention`.
    pub async fn open(path: &Path, retention: Duration) -> anyhow::Result<(Self, Restored)> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal);
        let pool = SqlitePool::connect_with(options)
            .await
            .with_context(|| format!("opening {}", path.display()))?;
        sqlx::migrate!()
            .run(&pool)
            .await
            .with_context(|| format!("migrating {}", path.display()))?;
        let restored = restore(&pool)
            .await
            .with_context(|| format!("reading {}", path.display()))?;
        let (writes, receiver) = mpsc::unbounded_channel();
        tokio::spawn(write_loop(pool.clone(), receiver, retention));
        Ok((Self { pool, writes }, restored))
    }

    /// Queued for the writer task; a failed write is logged there.
    pub fn write(&self, write: Write) {
        let _ = self.writes.send(write);
    }

    pub async fn history(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        since: Timestamp,
        bucket: Duration,
    ) -> anyhow::Result<History> {
        let since = since.as_millisecond();
        let bucket = i64::try_from(bucket.as_millis())?.max(1);
        let before = sqlx::query(
            "SELECT number FROM readings
             WHERE device_id = ? AND entity = ? AND channel = ? AND at < ? AND number IS NOT NULL
             ORDER BY at DESC LIMIT 1",
        )
        .bind(device_id)
        .bind(entity)
        .bind(channel)
        .bind(since)
        .fetch_optional(&self.pool)
        .await?
        .map(|row| row.get("number"));
        let mut buckets = Vec::new();
        for row in sqlx::query(
            "SELECT (at - ?) / ? AS bucket, AVG(number) AS average, MIN(number) AS minimum,
                    MAX(number) AS maximum
             FROM readings
             WHERE device_id = ? AND entity = ? AND channel = ? AND at >= ? AND number IS NOT NULL
             GROUP BY bucket ORDER BY bucket",
        )
        .bind(since)
        .bind(bucket)
        .bind(device_id)
        .bind(entity)
        .bind(channel)
        .bind(since)
        .fetch_all(&self.pool)
        .await?
        {
            buckets.push((
                usize::try_from(row.get::<i64, _>("bucket"))?,
                row.get("average"),
                row.get("minimum"),
                row.get("maximum"),
            ));
        }
        Ok(History { before, buckets })
    }

    /// Newest first.
    pub async fn recent_events(&self, limit: usize) -> anyhow::Result<Vec<(EventRecord, Timestamp)>> {
        let mut events = Vec::new();
        for row in sqlx::query(
            "SELECT device_id, entity, channel, at, kind, data FROM events ORDER BY at DESC LIMIT ?",
        )
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?
        {
            events.push(event_row(&row)?);
        }
        Ok(events)
    }

    /// Newest first.
    pub async fn recent_notifications(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredNotification>> {
        let mut notifications = Vec::new();
        for row in sqlx::query(
            "SELECT at, title, message, source FROM notifications ORDER BY at DESC LIMIT ?",
        )
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?
        {
            notifications.push(StoredNotification {
                title: row.get("title"),
                message: row.get("message"),
                source: row.get("source"),
                at: Timestamp::from_millisecond(row.get("at"))?,
            });
        }
        Ok(notifications)
    }

    /// Notifications about one node (its device id is their source), newest first.
    pub async fn notifications_about(
        &self,
        device_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredNotification>> {
        let mut notifications = Vec::new();
        for row in sqlx::query(
            "SELECT at, title, message, source FROM notifications WHERE source = ?
             ORDER BY at DESC LIMIT ?",
        )
        .bind(device_id)
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?
        {
            notifications.push(StoredNotification {
                title: row.get("title"),
                message: row.get("message"),
                source: row.get("source"),
                at: Timestamp::from_millisecond(row.get("at"))?,
            });
        }
        Ok(notifications)
    }

    /// Newest first.
    pub async fn updates(
        &self,
        device_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<StoredUpdate>> {
        let mut updates = Vec::new();
        for row in sqlx::query(
            "SELECT device_id, started_at, finished_at, from_version, to_version, outcome, detail
             FROM updates WHERE device_id = ? ORDER BY finished_at DESC LIMIT ?",
        )
        .bind(device_id)
        .bind(i64::try_from(limit)?)
        .fetch_all(&self.pool)
        .await?
        {
            updates.push(StoredUpdate {
                device_id: row.get("device_id"),
                started_at: Timestamp::from_millisecond(row.get("started_at"))?,
                finished_at: Timestamp::from_millisecond(row.get("finished_at"))?,
                from_version: row.get("from_version"),
                to_version: row.get("to_version"),
                outcome: UpdateOutcome::from_key(row.get("outcome"))?,
                detail: row.get("detail"),
            });
        }
        Ok(updates)
    }

    /// Waits for the queued writes.
    pub async fn flush(&self) {
        let (sender, receiver) = oneshot::channel();
        self.write(Write::Flush(sender));
        let _ = receiver.await;
    }
}

async fn write_loop(
    pool: SqlitePool,
    mut receiver: mpsc::UnboundedReceiver<Write>,
    retention: Duration,
) {
    let mut prune = tokio::time::interval(PRUNE_INTERVAL);
    loop {
        tokio::select! {
            write = receiver.recv() => {
                let Some(write) = write else { return };
                let mut batch = vec![write];
                while let Ok(write) = receiver.try_recv() {
                    batch.push(write);
                }
                if let Err(error) = write_batch(&pool, batch).await {
                    println!("storage: {error:#}");
                }
            }
            _ = prune.tick() => {
                if let Err(error) = prune_older(&pool, retention).await {
                    println!("storage: pruning: {error:#}");
                }
            }
        }
    }
}

/// One transaction for everything queued together.
async fn write_batch(pool: &SqlitePool, batch: Vec<Write>) -> anyhow::Result<()> {
    let mut flushed = Vec::new();
    let mut transaction = pool.begin().await?;
    for write in batch {
        match write {
            Write::Node(node) => write_node(&mut transaction, &node).await?,
            Write::Value(update, at) => write_value(&mut transaction, &update, at).await?,
            Write::Event(event, at) => write_event(&mut transaction, &event, at).await?,
            Write::RuleRun(name, run) => {
                sqlx::query(
                    "INSERT INTO rule_runs (name, at, errors) VALUES (?, ?, ?)
                     ON CONFLICT (name) DO UPDATE SET at = excluded.at, errors = excluded.errors",
                )
                .bind(name)
                .bind(run.at.as_millisecond())
                .bind(serde_json::to_string(&run.errors)?)
                .execute(&mut *transaction)
                .await?;
            }
            Write::Notification(notification) => {
                sqlx::query(
                    "INSERT INTO notifications (at, title, message, source) VALUES (?, ?, ?, ?)",
                )
                .bind(notification.at.as_millisecond())
                .bind(notification.title)
                .bind(notification.message)
                .bind(notification.source)
                .execute(&mut *transaction)
                .await?;
            }
            Write::Update(update) => {
                sqlx::query(
                    "INSERT INTO updates (device_id, started_at, finished_at, from_version,
                     to_version, outcome, detail) VALUES (?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(update.device_id)
                .bind(update.started_at.as_millisecond())
                .bind(update.finished_at.as_millisecond())
                .bind(update.from_version)
                .bind(update.to_version)
                .bind(update.outcome.key())
                .bind(update.detail)
                .execute(&mut *transaction)
                .await?;
            }
            Write::Flush(sender) => flushed.push(sender),
        }
    }
    transaction.commit().await?;
    for sender in flushed {
        let _ = sender.send(());
    }
    Ok(())
}

async fn write_node(
    transaction: &mut Transaction<'_, Sqlite>,
    node: &StoredNode,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO nodes (device_id, build_name, build_version, build_target, protocol_version,
                            entities, last_seen, uptime_seconds, rssi, heartbeat_interval_seconds)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (device_id) DO UPDATE SET
             build_name = excluded.build_name,
             protocol_version = excluded.protocol_version,
             build_version = excluded.build_version,
             build_target = excluded.build_target,
             entities = excluded.entities,
             last_seen = excluded.last_seen,
             uptime_seconds = excluded.uptime_seconds,
             rssi = excluded.rssi,
             heartbeat_interval_seconds = excluded.heartbeat_interval_seconds",
    )
    .bind(&node.device_id)
    .bind(&node.build.name)
    .bind(&node.build.version)
    .bind(&node.build.target)
    .bind(node.protocol_version)
    .bind(serde_json::to_string(&node.entities)?)
    .bind(node.last_seen.as_millisecond())
    .bind(node.uptime_seconds)
    .bind(node.rssi)
    .bind(i64::try_from(node.heartbeat_interval.as_secs())?)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn write_value(
    transaction: &mut Transaction<'_, Sqlite>,
    update: &ChannelUpdate,
    at: Timestamp,
) -> anyhow::Result<()> {
    let value = serde_json::to_string(&update.value)?;
    let confidence = variant_name(&update.confidence)?;
    sqlx::query(
        "INSERT INTO channel_values (device_id, entity, channel, value, confidence, at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT (device_id, entity, channel) DO UPDATE SET
             value = excluded.value, confidence = excluded.confidence, at = excluded.at",
    )
    .bind(&update.device_id)
    .bind(update.entity)
    .bind(update.channel)
    .bind(&value)
    .bind(&confidence)
    .bind(at.as_millisecond())
    .execute(&mut **transaction)
    .await?;
    sqlx::query(
        "INSERT INTO readings (device_id, entity, channel, at, value, number, confidence)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&update.device_id)
    .bind(update.entity)
    .bind(update.channel)
    .bind(at.as_millisecond())
    .bind(&value)
    .bind(number(&update.value))
    .bind(&confidence)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn write_event(
    transaction: &mut Transaction<'_, Sqlite>,
    event: &EventRecord,
    at: Timestamp,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO events (device_id, entity, channel, at, kind, data) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&event.device_id)
    .bind(event.entity)
    .bind(event.channel)
    .bind(at.as_millisecond())
    .bind(variant_name(&event.kind)?)
    .bind(event.data.as_ref().map(serde_json::to_string).transpose()?)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

async fn prune_older(pool: &SqlitePool, retention: Duration) -> anyhow::Result<()> {
    let before = (Timestamp::now() - SignedDuration::try_from(retention)?).as_millisecond();
    let readings = sqlx::query("DELETE FROM readings WHERE at < ?")
        .bind(before)
        .execute(pool)
        .await?
        .rows_affected();
    let events = sqlx::query("DELETE FROM events WHERE at < ?")
        .bind(before)
        .execute(pool)
        .await?
        .rows_affected();
    let notifications = sqlx::query("DELETE FROM notifications WHERE at < ?")
        .bind(before)
        .execute(pool)
        .await?
        .rows_affected();
    if readings + events + notifications > 0 {
        println!(
            "storage: pruned {readings} readings, {events} events and {notifications} notifications"
        );
    }
    Ok(())
}

async fn restore(pool: &SqlitePool) -> anyhow::Result<Restored> {
    let mut restored = Restored::default();

    for row in sqlx::query(
        "SELECT device_id, build_name, build_version, build_target, protocol_version, entities, last_seen, uptime_seconds, rssi,
                heartbeat_interval_seconds
         FROM nodes",
    )
    .fetch_all(pool)
    .await?
    {
        let entities: Vec<EntityDescriptor> = serde_json::from_str(row.get("entities"))?;
        restored.nodes.push(StoredNode {
            device_id: row.get("device_id"),
            build: Build::new(
                row.get::<String, _>("build_name"),
                row.get::<String, _>("build_version"),
                row.get::<String, _>("build_target"),
            ),
            protocol_version: row.get("protocol_version"),
            entities,
            last_seen: Timestamp::from_millisecond(row.get("last_seen"))?,
            uptime_seconds: row.get("uptime_seconds"),
            rssi: row.get("rssi"),
            heartbeat_interval: Duration::from_secs(
                row.get::<i64, _>("heartbeat_interval_seconds").try_into()?,
            ),
        });
    }

    for row in
        sqlx::query("SELECT device_id, entity, channel, value, confidence FROM channel_values")
            .fetch_all(pool)
            .await?
    {
        restored.values.push((
            row.get("device_id"),
            row.get("entity"),
            row.get("channel"),
            serde_json::from_str(row.get("value"))?,
            from_variant_name(row.get("confidence"))?,
        ));
    }

    // SQLite takes the other columns from the row with the latest `at`.
    for row in sqlx::query(
        "SELECT device_id, entity, channel, MAX(at) AS at, kind, data
         FROM events GROUP BY device_id, entity, channel",
    )
    .fetch_all(pool)
    .await?
    {
        restored.last_events.push(event_row(&row)?);
    }

    for row in sqlx::query(
        "SELECT device_id, entity, channel, at, kind, data
         FROM events WHERE kind = ? ORDER BY at DESC LIMIT ?",
    )
    .bind(variant_name(&EventKind::Received)?)
    .bind(RECEIVED_EVENTS)
    .fetch_all(pool)
    .await?
    {
        restored.received_events.push(event_row(&row)?);
    }

    for row in sqlx::query("SELECT name, at, errors FROM rule_runs")
        .fetch_all(pool)
        .await?
    {
        restored.rule_runs.push((
            row.get("name"),
            RuleRun {
                at: Timestamp::from_millisecond(row.get("at"))?,
                errors: serde_json::from_str(row.get("errors"))?,
            },
        ));
    }

    Ok(restored)
}

fn event_row(row: &sqlx::sqlite::SqliteRow) -> anyhow::Result<(EventRecord, Timestamp)> {
    let data: Option<&str> = row.get("data");
    Ok((
        EventRecord {
            device_id: row.get("device_id"),
            entity: row.get("entity"),
            channel: row.get("channel"),
            kind: from_variant_name(row.get("kind"))?,
            data: data.map(serde_json::from_str).transpose()?,
        },
        Timestamp::from_millisecond(row.get("at"))?,
    ))
}

/// The value as a number for charts: Bool, Int and Float.
fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(on) => Some(f64::from(u8::from(*on))),
        Value::Int(number) => Some(f64::from(*number)),
        Value::Float(number) => Some(f64::from(*number)),
        _ => None,
    }
}

/// A unit variant's name, e.g. `Confirmed`.
fn variant_name<T: Serialize>(variant: &T) -> anyhow::Result<String> {
    match serde_json::to_value(variant)? {
        serde_json::Value::String(name) => Ok(name),
        other => anyhow::bail!("{other} is not a unit variant"),
    }
}

fn from_variant_name<T: DeserializeOwned>(name: &str) -> anyhow::Result<T> {
    Ok(serde_json::from_value(serde_json::Value::String(
        name.to_string(),
    ))?)
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::Confidence;

    use super::*;

    #[test]
    fn variant_names_round_trip() {
        let name = variant_name(&Confidence::Assumed).unwrap();
        assert_eq!(name, "Assumed");
        assert_eq!(
            from_variant_name::<Confidence>(&name).unwrap(),
            Confidence::Assumed
        );
        assert_eq!(variant_name(&EventKind::LongPress).unwrap(), "LongPress");
    }

    #[tokio::test]
    async fn restart_restores_what_was_written() {
        let directory =
            std::env::temp_dir().join(format!("oxihome-storage-test-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("hub.sqlite");
        let _ = std::fs::remove_file(&path);
        let at = Timestamp::now();

        let (storage, restored) = Storage::open(&path, Duration::from_secs(3600))
            .await
            .unwrap();
        assert!(restored.nodes.is_empty());
        storage.write(Write::Node(StoredNode {
            device_id: "esp32-test".into(),
            build: Build::new("firmware", "0.9.0", "esp32"),
            protocol_version: 2,
            entities: Vec::new(),
            last_seen: at,
            uptime_seconds: Some(120),
            rssi: Some(-60),
            heartbeat_interval: Duration::from_secs(30),
        }));
        storage.write(Write::Value(
            ChannelUpdate {
                device_id: "esp32-test".into(),
                entity: 1,
                channel: 0,
                value: Value::Float(21.5),
                confidence: Confidence::Confirmed,
                previous: None,
            },
            at,
        ));
        let received = EventRecord {
            device_id: "esp32-test".into(),
            entity: 6,
            channel: 0,
            kind: EventKind::Received,
            data: Some(Value::Text("NEC 00 45".into())),
        };
        storage.write(Write::Event(received.clone(), at));
        storage.write(Write::RuleRun(
            "Night".into(),
            RuleRun {
                at,
                errors: vec!["failed".into()],
            },
        ));
        storage.write(Write::Notification(StoredNotification {
            title: "Test board offline".into(),
            message: "Stopped sending messages".into(),
            source: "esp32-test".into(),
            at,
        }));
        storage.flush().await;
        assert_eq!(storage.recent_events(5).await.unwrap().len(), 1);
        assert_eq!(
            storage.recent_notifications(5).await.unwrap()[0].title,
            "Test board offline"
        );
        let history = storage
            .history(
                "esp32-test",
                1,
                0,
                at - SignedDuration::from_mins(10),
                Duration::from_secs(60),
            )
            .await
            .unwrap();
        assert_eq!(
            history,
            History {
                before: None,
                buckets: vec![(10, 21.5, 21.5, 21.5)],
            }
        );

        let (_, restored) = Storage::open(&path, Duration::from_secs(3600))
            .await
            .unwrap();
        assert_eq!(restored.nodes.len(), 1);
        assert_eq!(restored.nodes[0].uptime_seconds, Some(120));
        assert_eq!(
            restored.values,
            vec![(
                "esp32-test".into(),
                1,
                0,
                Value::Float(21.5),
                Confidence::Confirmed
            )]
        );
        let at_milliseconds = Timestamp::from_millisecond(at.as_millisecond()).unwrap();
        assert_eq!(
            restored.last_events,
            vec![(received.clone(), at_milliseconds)]
        );
        assert_eq!(restored.received_events, vec![(received, at_milliseconds)]);
        assert_eq!(restored.rule_runs[0].1.errors, vec!["failed".to_string()]);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[tokio::test]
    async fn updates_and_notifications_by_node() {
        let directory = std::env::temp_dir().join(format!(
            "oxihome-storage-updates-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("hub.sqlite");
        let _ = std::fs::remove_file(&path);
        let (storage, _) = Storage::open(&path, Duration::from_secs(3600))
            .await
            .unwrap();
        let at = Timestamp::from_millisecond(Timestamp::now().as_millisecond()).unwrap();
        let update = StoredUpdate {
            device_id: "esp32-a".into(),
            started_at: at,
            finished_at: at,
            from_version: "0.12.1".into(),
            to_version: "0.12.2".into(),
            outcome: UpdateOutcome::RolledBack,
            detail: None,
        };
        storage.write(Write::Update(update.clone()));
        for source in ["esp32-a", "esp32-b"] {
            storage.write(Write::Notification(StoredNotification {
                title: format!("{source} offline"),
                message: "Stopped sending messages".into(),
                source: source.into(),
                at,
            }));
        }
        storage.flush().await;
        assert_eq!(storage.updates("esp32-a", 10).await.unwrap(), vec![update]);
        assert!(storage.updates("esp32-b", 10).await.unwrap().is_empty());
        let about_a = storage.notifications_about("esp32-a", 10).await.unwrap();
        assert_eq!(about_a.len(), 1);
        assert_eq!(about_a[0].title, "esp32-a offline");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
