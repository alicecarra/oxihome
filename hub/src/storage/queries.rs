//! What the web interface reads back: histories, recent events and notifications, updates and
//! the timeline.

use std::time::Duration;

use jiff::Timestamp;
use sqlx::Row;

use super::{
    History, Operation, Storage, StoredCommand, StoredNotification, StoredUpdate, TimelineEntry,
    TimelineKind, TimelineQuery, UpdateOutcome, event_row,
};
use crate::config::rules::RuleRun;
use crate::node::events::EventRecord;

impl Storage {
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
    pub async fn recent_events(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<(EventRecord, Timestamp)>> {
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

    /// One node's events, newest first.
    pub async fn events_of(
        &self,
        device_id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<(EventRecord, Timestamp)>> {
        let mut events = Vec::new();
        for row in sqlx::query(
            "SELECT device_id, entity, channel, at, kind, data FROM events WHERE device_id = ?
             ORDER BY at DESC LIMIT ?",
        )
        .bind(device_id)
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

    /// Newest first. Held IR keys (` repeat` codes) are left out.
    pub async fn timeline(&self, query: &TimelineQuery) -> anyhow::Result<Vec<TimelineEntry>> {
        let wanted = |kind: TimelineKind| query.kind.is_none_or(|chosen| chosen == kind);
        let before = query.before.map_or(i64::MAX, Timestamp::as_millisecond);
        let device_id = query.device_id.as_deref();
        let rule = query.rule.as_deref();
        let limit = i64::try_from(query.limit)?;
        let mut entries = Vec::new();
        if wanted(TimelineKind::Events) && rule.is_none() {
            for row in self
                .timeline_rows(
                    "SELECT device_id, entity, channel, at, kind, data FROM events
                     WHERE at < ? {filter} AND NOT (kind = 'Received' AND COALESCE(data, '') LIKE '% repeat\"}')
                     ORDER BY at DESC LIMIT ?",
                    &[("device_id", device_id)],
                    before,
                    limit,
                )
                .await?
            {
                let (event, at) = event_row(&row)?;
                entries.push(TimelineEntry::Event(event, at));
            }
        }
        if wanted(TimelineKind::Rules) && device_id.is_none() {
            for row in self
                .timeline_rows(
                    "SELECT name, at, errors FROM rule_executions WHERE at < ? {filter}
                     ORDER BY at DESC LIMIT ?",
                    &[("name", rule)],
                    before,
                    limit,
                )
                .await?
            {
                entries.push(TimelineEntry::RuleRun(
                    row.get("name"),
                    RuleRun {
                        at: Timestamp::from_millisecond(row.get("at"))?,
                        errors: serde_json::from_str(row.get("errors"))?,
                    },
                ));
            }
        }
        if wanted(TimelineKind::Commands) {
            for row in self
                .timeline_rows(
                    "SELECT device_id, entity, channel, at, operation, value, rule, error
                     FROM commands WHERE at < ? {filter} ORDER BY at DESC LIMIT ?",
                    &[("device_id", device_id), ("rule", rule)],
                    before,
                    limit,
                )
                .await?
            {
                let value: Option<&str> = row.get("value");
                entries.push(TimelineEntry::Command(StoredCommand {
                    device_id: row.get("device_id"),
                    entity: row.get("entity"),
                    channel: row.get("channel"),
                    at: Timestamp::from_millisecond(row.get("at"))?,
                    operation: Operation::from_key(row.get("operation"))?,
                    value: value.map(serde_json::from_str).transpose()?,
                    rule: row.get("rule"),
                    error: row.get("error"),
                }));
            }
        }
        if wanted(TimelineKind::Notifications) {
            for row in self
                .timeline_rows(
                    "SELECT at, title, message, source FROM notifications WHERE at < ? {filter}
                     ORDER BY at DESC LIMIT ?",
                    &[("source", device_id), ("source", rule)],
                    before,
                    limit,
                )
                .await?
            {
                entries.push(TimelineEntry::Notification(StoredNotification {
                    title: row.get("title"),
                    message: row.get("message"),
                    source: row.get("source"),
                    at: Timestamp::from_millisecond(row.get("at"))?,
                }));
            }
        }
        if wanted(TimelineKind::Updates) && rule.is_none() {
            for row in self
                .timeline_rows(
                    "SELECT device_id, started_at, finished_at, from_version, to_version, outcome,
                            detail
                     FROM updates WHERE finished_at < ? {filter} ORDER BY finished_at DESC LIMIT ?",
                    &[("device_id", device_id)],
                    before,
                    limit,
                )
                .await?
            {
                entries.push(TimelineEntry::Update(StoredUpdate {
                    device_id: row.get("device_id"),
                    started_at: Timestamp::from_millisecond(row.get("started_at"))?,
                    finished_at: Timestamp::from_millisecond(row.get("finished_at"))?,
                    from_version: row.get("from_version"),
                    to_version: row.get("to_version"),
                    outcome: UpdateOutcome::from_key(row.get("outcome"))?,
                    detail: row.get("detail"),
                }));
            }
        }
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.at()));
        entries.truncate(query.limit);
        Ok(entries)
    }

    /// Runs `sql` with `{filter}` replaced by `AND column = ?` for each column whose value is set.
    async fn timeline_rows(
        &self,
        sql: &str,
        filters: &[(&str, Option<&str>)],
        before: i64,
        limit: i64,
    ) -> anyhow::Result<Vec<sqlx::sqlite::SqliteRow>> {
        let filters: Vec<(&str, &str)> = filters
            .iter()
            .filter_map(|(column, value)| Some((*column, (*value)?)))
            .collect();
        let filter: String = filters
            .iter()
            .map(|(column, _)| format!("AND {column} = ? "))
            .collect();
        let sql = sql.replace("{filter}", &filter);
        // The filter only names columns given by the code.
        let mut query = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(before);
        for (_, value) in filters {
            query = query.bind(value);
        }
        Ok(query.bind(limit).fetch_all(&self.pool).await?)
    }
}
