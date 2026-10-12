//! The hub's state and history in SQLite, so a restart loses almost nothing.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use jiff::{SignedDuration, Timestamp};
use oxihome_protocol::{Confidence, EntityDescriptor, EventKind, Value};
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

pub struct Storage {
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
        tokio::spawn(write_loop(pool, receiver, retention));
        Ok((Self { writes }, restored))
    }

    /// Queued for the writer task; a failed write is logged there.
    pub fn write(&self, write: Write) {
        let _ = self.writes.send(write);
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
        "INSERT INTO nodes (device_id, firmware_version, entities, last_seen, uptime_seconds, rssi,
                            heartbeat_interval_seconds)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (device_id) DO UPDATE SET
             firmware_version = excluded.firmware_version,
             entities = excluded.entities,
             last_seen = excluded.last_seen,
             uptime_seconds = excluded.uptime_seconds,
             rssi = excluded.rssi,
             heartbeat_interval_seconds = excluded.heartbeat_interval_seconds",
    )
    .bind(&node.device_id)
    .bind(&node.firmware_version)
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
    if readings + events > 0 {
        println!("storage: pruned {readings} readings and {events} events");
    }
    Ok(())
}

async fn restore(pool: &SqlitePool) -> anyhow::Result<Restored> {
    let mut restored = Restored::default();

    for row in sqlx::query(
        "SELECT device_id, firmware_version, entities, last_seen, uptime_seconds, rssi,
                heartbeat_interval_seconds
         FROM nodes",
    )
    .fetch_all(pool)
    .await?
    {
        let entities: Vec<EntityDescriptor> = serde_json::from_str(row.get("entities"))?;
        restored.nodes.push(StoredNode {
            device_id: row.get("device_id"),
            firmware_version: row.get("firmware_version"),
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
            firmware_version: "0.9.0".into(),
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
        storage.flush().await;

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
}
