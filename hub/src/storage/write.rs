//! The writer task: queued writes go to SQLite in batches, and old history is pruned.

use std::time::Duration;

use jiff::{SignedDuration, Timestamp};
use sqlx::sqlite::SqlitePool;
use sqlx::{Sqlite, Transaction};
use tokio::sync::mpsc;

use super::{Write, number, variant_name};
use crate::node::events::EventRecord;
use crate::node::registry::StoredNode;
use crate::node::state::ChannelUpdate;

const PRUNE_INTERVAL: Duration = Duration::from_secs(3600);

pub(super) async fn write_loop(
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
                write_batch(&pool, batch).await;
            }
            _ = prune.tick() => {
                if let Err(error) = prune_older(&pool, retention).await {
                    println!("storage: pruning: {error:#}");
                }
            }
        }
    }
}

/// One transaction for everything queued together. When it fails, each write is retried on its
/// own, so a bad one does not take the others down with it.
async fn write_batch(pool: &SqlitePool, batch: Vec<Write>) {
    let mut flushed = Vec::new();
    let mut writes = Vec::new();
    for write in batch {
        match write {
            Write::Flush(sender) => flushed.push(sender),
            write => writes.push(write),
        }
    }
    if let Err(error) = write_all(pool, &writes).await {
        if writes.len() == 1 {
            println!("storage: {} dropped: {error:#}", writes[0].kind());
        } else {
            println!(
                "storage: {} writes failed together ({error:#}); writing them one by one",
                writes.len()
            );
            for write in &writes {
                if let Err(error) = write_all(pool, std::slice::from_ref(write)).await {
                    println!("storage: {} dropped: {error:#}", write.kind());
                }
            }
        }
    }
    for sender in flushed {
        let _ = sender.send(());
    }
}

async fn write_all(pool: &SqlitePool, writes: &[Write]) -> anyhow::Result<()> {
    let mut transaction = pool.begin().await?;
    for write in writes {
        write_one(&mut transaction, write).await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn write_one(transaction: &mut Transaction<'_, Sqlite>, write: &Write) -> anyhow::Result<()> {
    match write {
        Write::Node(node) => write_node(transaction, node).await?,
        Write::Value(update, at) => write_value(transaction, update, *at).await?,
        Write::Event(event, at) => write_event(transaction, event, *at).await?,
        Write::WaitBegan(wait) => {
            sqlx::query(
                "INSERT INTO rule_waits (rule, device_id, trigger, deadline, subject)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT (rule, device_id) DO UPDATE SET
                     trigger = excluded.trigger, deadline = excluded.deadline,
                     subject = excluded.subject",
            )
            .bind(&wait.rule)
            .bind(&wait.device_id)
            .bind(&wait.trigger)
            .bind(wait.deadline.as_millisecond())
            .bind(serde_json::to_string(&wait.subject)?)
            .execute(&mut **transaction)
            .await?;
        }
        Write::WaitEnded(rule, device_id) => {
            sqlx::query("DELETE FROM rule_waits WHERE rule = ? AND device_id = ?")
                .bind(rule)
                .bind(device_id)
                .execute(&mut **transaction)
                .await?;
        }
        Write::RuleRun(name, run) => {
            sqlx::query(
                "INSERT INTO rule_runs (name, at, errors) VALUES (?, ?, ?)
                 ON CONFLICT (name) DO UPDATE SET at = excluded.at, errors = excluded.errors",
            )
            .bind(name)
            .bind(run.at.as_millisecond())
            .bind(serde_json::to_string(&run.errors)?)
            .execute(&mut **transaction)
            .await?;
            sqlx::query("INSERT INTO rule_executions (name, at, errors) VALUES (?, ?, ?)")
                .bind(name)
                .bind(run.at.as_millisecond())
                .bind(serde_json::to_string(&run.errors)?)
                .execute(&mut **transaction)
                .await?;
        }
        Write::Command(command) => {
            sqlx::query(
                "INSERT INTO commands (device_id, entity, channel, at, operation, value, rule,
                 error) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&command.device_id)
            .bind(command.entity)
            .bind(command.channel)
            .bind(command.at.as_millisecond())
            .bind(command.operation.key())
            .bind(
                command
                    .value
                    .as_ref()
                    .map(serde_json::to_string)
                    .transpose()?,
            )
            .bind(&command.rule)
            .bind(&command.error)
            .execute(&mut **transaction)
            .await?;
        }
        Write::Notification(notification) => {
            sqlx::query(
                "INSERT INTO notifications (at, title, message, source) VALUES (?, ?, ?, ?)",
            )
            .bind(notification.at.as_millisecond())
            .bind(&notification.title)
            .bind(&notification.message)
            .bind(&notification.source)
            .execute(&mut **transaction)
            .await?;
        }
        Write::Update(update) => {
            sqlx::query(
                "INSERT INTO updates (device_id, started_at, finished_at, from_version,
                 to_version, outcome, detail) VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&update.device_id)
            .bind(update.started_at.as_millisecond())
            .bind(update.finished_at.as_millisecond())
            .bind(&update.from_version)
            .bind(&update.to_version)
            .bind(update.outcome.key())
            .bind(&update.detail)
            .execute(&mut **transaction)
            .await?;
        }
        Write::Forget(device_id) => {
            for query in [
                "DELETE FROM nodes WHERE device_id = ?",
                "DELETE FROM channel_values WHERE device_id = ?",
                "DELETE FROM updates WHERE device_id = ?",
            ] {
                sqlx::query(query)
                    .bind(device_id)
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        Write::ForgetRuleRuns(names) => {
            for name in names {
                sqlx::query("DELETE FROM rule_runs WHERE name = ?")
                    .bind(name)
                    .execute(&mut **transaction)
                    .await?;
            }
        }
        Write::Flush(_) => {}
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
    let rule_runs = sqlx::query("DELETE FROM rule_executions WHERE at < ?")
        .bind(before)
        .execute(pool)
        .await?
        .rows_affected();
    let commands = sqlx::query("DELETE FROM commands WHERE at < ?")
        .bind(before)
        .execute(pool)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM sessions WHERE expires_at < ?")
        .bind(Timestamp::now().as_millisecond())
        .execute(pool)
        .await?;
    if readings + events + notifications + rule_runs + commands > 0 {
        println!(
            "storage: pruned {readings} readings, {events} events, {notifications} notifications, \
             {rule_runs} rule runs and {commands} commands"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::{Build, Value};

    use super::*;
    use crate::storage::{Operation, Storage, StoredCommand, StoredNotification};

    #[tokio::test]
    async fn a_failed_write_does_not_drop_the_rest_of_its_batch() {
        let directory =
            std::env::temp_dir().join(format!("oxihome-storage-batch-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("hub.sqlite");
        let _ = std::fs::remove_file(&path);
        let (storage, _) = Storage::open(&path, Duration::from_secs(3600))
            .await
            .unwrap();
        sqlx::query("DROP TABLE commands")
            .execute(&storage.pool)
            .await
            .unwrap();
        let at = Timestamp::now();

        write_batch(
            &storage.pool,
            vec![
                Write::Node(StoredNode {
                    device_id: "esp32-test".into(),
                    build: Build::new("firmware", "0.9.0", "esp32"),
                    protocol_version: 2,
                    entities: Vec::new(),
                    last_seen: at,
                    uptime_seconds: None,
                    rssi: None,
                    heartbeat_interval: Duration::from_secs(30),
                }),
                Write::Command(StoredCommand {
                    device_id: "esp32-test".into(),
                    entity: 1,
                    channel: 0,
                    at,
                    operation: Operation::Set,
                    value: Some(Value::Bool(true)),
                    rule: None,
                    error: None,
                }),
                Write::Notification(StoredNotification {
                    title: "Saved".into(),
                    message: String::new(),
                    source: "test".into(),
                    at,
                }),
            ],
        )
        .await;

        let nodes: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM nodes")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        let notifications: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notifications")
            .fetch_one(&storage.pool)
            .await
            .unwrap();
        assert_eq!((nodes, notifications), (1, 1));
    }
}
