//! What the hub does with each thing that happens: a broker connection, a message, a config change, a presence check, a clock tick.

use std::sync::Arc;
use std::time::Instant;

use jiff::Timestamp;
use oxihome_protocol::{ErrorCode, Message, Payload, QueryKind, management};

use crate::config::rules::{self, Rule, Subject};
use crate::hub::Hub;
use crate::node::registry::NodeEvent;
use crate::node::{hardware, parameters};
use crate::schedule::Schedule;
use crate::traffic::{Alert, AlertKind, RaisedAlert};

/// (Re)connected to the broker: ask every node to introduce itself and report its state.
pub async fn on_connected(hub: &Hub) -> anyhow::Result<()> {
    let mqtt = hub.mqtt();
    mqtt.broadcast(Payload::Query {
        what: QueryKind::Hello,
    })
    .await?;
    mqtt.broadcast(Payload::Query {
        what: QueryKind::State { entity: None },
    })
    .await
}

/// What an `Error` a node sent on its own says about its update, when it is about one. The board
/// acknowledges the `Ota` command before downloading, so the failure comes later. Firmware before
/// 0.19.0 sends `Internal` with an "OTA:" detail.
fn update_failure(code: u16, detail: Option<&str>) -> Option<String> {
    let detail = detail.unwrap_or("no detail");
    match ErrorCode::try_from(code) {
        Ok(ErrorCode::UpdateDownloadFailed) => Some(format!(
            "download failed ({detail}); check [ota] public_url and that the board reaches its port"
        )),
        Ok(ErrorCode::UpdateRejected) => Some(format!("image rejected ({detail})")),
        Ok(ErrorCode::Internal) if detail.starts_with("OTA:") => Some(detail.to_string()),
        _ => None,
    }
}

pub async fn on_message(hub: &Arc<Hub>, message: &Message) -> anyhow::Result<()> {
    let (node_events, updates, event) = hub.handle(message, Instant::now());
    match &message.payload {
        Payload::Hello { build, .. } => {
            hub.settle_update(&message.src, &build.version);
            hub.hardware_restarted(&message.src);
        }
        Payload::Error { code, detail } if message.reply_to.is_none() => {
            if let Some(failure) = update_failure(*code, detail.as_deref()) {
                hub.fail_update(&message.src, &failure);
            } else if let Some(detail) = detail {
                hub.record_node_report(&message.src, detail);
                let report = RaisedAlert {
                    alert: Alert {
                        kind: AlertKind::Reported,
                        device_id: message.src.clone(),
                        channel: None,
                    },
                    detail: detail.clone(),
                };
                on_alert(hub, &report);
            }
        }
        _ => {}
    }

    let reports_parameters = matches!(
        message.payload,
        Payload::State {
            entity: management::ENTITY,
            ..
        }
    );
    // After a `Hello` the reported state is stale; the `State` the node resends applies them.
    let came_back = !matches!(message.payload, Payload::Hello { .. })
        && node_events
            .iter()
            .any(|node_event| matches!(node_event, NodeEvent::Online(_)));
    if reports_parameters || came_back {
        tokio::spawn(parameters::apply(hub.clone(), message.src.clone()));
        tokio::spawn(hardware::apply(hub.clone(), message.src.clone()));
    }

    for node_event in node_events {
        on_node_event(hub, &node_event);
        if let NodeEvent::Unknown(device_id) = node_event {
            let mqtt = hub.mqtt();
            mqtt.publish(
                mqtt.next_message_id(),
                Some(&device_id),
                Payload::Query {
                    what: QueryKind::Hello,
                },
            )
            .await?;
        }
    }

    for update in updates {
        println!("{update}");
        spawn_rules(
            hub,
            hub.crossed_rules(&update),
            Subject::of_update(&update, hub.as_ref()),
        );
    }

    if let Some(event) = event {
        println!("{event}");
        spawn_rules(
            hub,
            hub.matching_rules(&event),
            Subject::of_event(&event, hub.as_ref()),
        );
    }
    Ok(())
}

/// A config file changed on disk: apply it if it is valid, keep the current one otherwise.
pub fn on_config_changed(hub: &Arc<Hub>) {
    match hub.reload_config_if_changed() {
        None => {}
        Some(Ok(rule_count)) => {
            println!("config reloaded from disk ({rule_count} rules)");
            hub.notify(
                "Config reloaded from disk".into(),
                format!("{rule_count} rules"),
                "settings".into(),
            );
        }
        Some(Err(error)) => {
            println!("config not reloaded: {error:#}");
            hub.notify(
                "Config not reloaded".into(),
                format!("{error:#}"),
                "settings".into(),
            );
        }
    }
}

/// Marks nodes that went quiet as offline.
pub fn on_presence_check(hub: &Arc<Hub>) {
    hub.check_updates(Instant::now());
    for node_event in hub.check_presence(Instant::now()) {
        on_node_event(hub, &node_event);
    }
    // A node rebooting into an update goes quiet for a while, like an offline one.
    for raised in hub
        .traffic()
        .new_alerts(|device_id| hub.is_online(device_id) && !hub.is_updating(device_id))
    {
        on_alert(hub, &raised);
    }
}

fn on_alert(hub: &Arc<Hub>, raised: &RaisedAlert) {
    let alert = &raised.alert;
    let node = hub.node_name(&alert.device_id);
    let channel = alert
        .channel
        .map(|(entity, channel)| hub.channel_name(&alert.device_id, entity, channel));
    let about = match &channel {
        Some(channel) => format!("{node} › {channel}"),
        None => node.clone(),
    };
    let title = match alert.kind {
        AlertKind::Stopped => format!("{about} stopped reporting"),
        AlertKind::Noisy => format!("{about} is noisy"),
        AlertKind::Unanswered => format!("{about} is not answering commands"),
        AlertKind::Reported => format!("{about} reported a problem"),
    };
    println!("{title}: {}", raised.detail);
    hub.notify(title, raised.detail.clone(), alert.device_id.clone());
    let subject = Subject {
        node: Some(node),
        channel,
        detail: Some(raised.detail.clone()),
        value: None,
    };
    spawn_rules(hub, hub.traffic_rules(alert), subject);
}

/// Runs the scheduled rules of each new local minute.
pub fn on_clock_tick(hub: &Arc<Hub>, schedule: &mut Schedule) {
    for minute in schedule.new_minutes(Timestamp::now()) {
        spawn_rules(hub, hub.due_rules(minute), Subject::default());
    }
}

fn on_node_event(hub: &Arc<Hub>, node_event: &NodeEvent) {
    println!("{node_event}");
    hub.notify_node_event(node_event);
    if let NodeEvent::Offline(device_id) = node_event {
        let subject = Subject {
            node: Some(hub.node_name(device_id)),
            ..Subject::default()
        };
        spawn_rules(hub, hub.offline_rules(device_id), subject);
    }
}

fn spawn_rules(hub: &Arc<Hub>, matched: Vec<Rule>, subject: Subject) {
    for rule in matched {
        rules::start(hub.clone(), rule, subject.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxihome_protocol::management::{self, HEARTBEAT_INTERVAL};
    use oxihome_protocol::{Build, CommandOp, Value};

    use super::*;
    use crate::hub::CommandOrigin;

    #[tokio::test]
    async fn config_changed_on_disk_is_applied_once_and_only_if_valid() {
        let hub = Hub::for_tests("config-changed", "").await;
        let path = hub.config_directory().join("rules.toml");
        let rule = |name: &str| {
            format!(
                "[[rules]]\nname = \"{name}\"\nwhen = {{ device = \"sim\", entity = 1, channel = 0, event = \"Press\" }}\nthen = []\n"
            )
        };
        let titles = || -> Vec<String> {
            hub.notifications_since(0)
                .into_iter()
                .map(|notification| notification.title)
                .collect()
        };

        std::fs::write(&path, rule("First")).unwrap();
        on_config_changed(&hub);
        on_config_changed(&hub);
        assert_eq!(hub.rules().len(), 1);
        assert_eq!(titles(), ["Config reloaded from disk"]);

        std::fs::write(&path, "[[rules]]\nname = 3\n").unwrap();
        on_config_changed(&hub);
        on_config_changed(&hub);
        assert_eq!(hub.rules()[0].name, "First");
        assert_eq!(
            titles(),
            ["Config reloaded from disk", "Config not reloaded"]
        );

        let modified = hub.config_file("rules.toml").unwrap().modified;
        let two_rules = format!("{}{}", rule("First"), rule("Second"));
        hub.save_config_file("rules.toml", &two_rules, modified)
            .unwrap();
        on_config_changed(&hub);
        assert_eq!(hub.rules().len(), 2);
        assert_eq!(titles().len(), 2, "the hub's own write was reloaded again");
    }

    #[tokio::test]
    async fn a_rule_waits_and_ignores_triggers_while_running() {
        let hub = Hub::for_tests("waits", "").await;
        let rule: Rule = toml::from_str(
            r#"
            name = "Waits"
            when = { button = "Go" }
            then = [
                { notify = { title = "First", message = "" } },
                { wait = "200ms" },
                { notify = { title = "Second", message = "" } },
            ]
            "#,
        )
        .unwrap();
        let titles = || -> Vec<String> {
            hub.notifications_since(0)
                .into_iter()
                .map(|notification| notification.title)
                .collect()
        };
        let rules::Start::Started(actions) =
            rules::start(hub.clone(), rule.clone(), Subject::default())
        else {
            panic!("the rule starts");
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(titles(), ["First"]);
        assert!(hub.running().includes("Waits"));
        assert!(matches!(
            rules::start(hub.clone(), rule.clone(), Subject::default()),
            rules::Start::Busy
        ));
        let other_node = Subject {
            node: Some("Other".into()),
            ..Subject::default()
        };
        let rules::Start::Started(other) = rules::start(hub.clone(), rule.clone(), other_node)
        else {
            panic!("another node is another run");
        };
        actions.await.unwrap();
        other.await.unwrap();
        assert_eq!(titles(), ["First", "First", "Second", "Second"]);
        assert!(!hub.running().includes("Waits"));
        assert!(matches!(
            rules::start(hub.clone(), rule, Subject::default()),
            rules::Start::Started(_)
        ));
    }

    fn hello(device_id: &str) -> Message {
        Message::new(
            device_id,
            0,
            Payload::Hello {
                build: Build::new("firmware", "0.9.0", "esp32"),
                ip: None,
                entities: Vec::new(),
            },
        )
    }

    fn ack(device_id: &str, message_id: u32) -> Message {
        Message::new(device_id, 1, Payload::Ack { ok: true }).replying_to(message_id)
    }

    #[tokio::test]
    async fn saved_parameters_are_sent_to_the_node() {
        let hub = Hub::for_tests("apply-config", "").await;
        let mut lamp = hello("lamp");
        if let Payload::Hello { entities, .. } = &mut lamp.payload {
            entities.push(management::descriptor());
        }
        on_message(&hub, &lamp).await.unwrap();

        let mut sent = hub.traffic().subscribe();
        let modified = hub.config_file("nodes.toml").unwrap().modified;
        hub.save_config_file(
            "nodes.toml",
            "[nodes.lamp]\nparameters = { heartbeat_interval = 60 }\n",
            modified,
        )
        .unwrap();
        let entry = tokio::time::timeout(Duration::from_secs(1), sent.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(entry.device_id.as_deref(), Some("lamp"));
        assert_eq!(
            Message::from_json(&entry.text).unwrap().payload,
            Payload::Command {
                entity: management::ENTITY,
                channel: HEARTBEAT_INTERVAL.channel.into(),
                op: CommandOp::Set(Value::Int(60)),
            }
        );
    }

    #[test]
    fn update_failures_by_code() {
        let download = update_failure(ErrorCode::UpdateDownloadFailed.into(), Some("HTTP 404"));
        assert!(download.unwrap().starts_with("download failed (HTTP 404)"));
        assert_eq!(
            update_failure(ErrorCode::UpdateRejected.into(), None).as_deref(),
            Some("image rejected (no detail)")
        );
        assert_eq!(
            update_failure(ErrorCode::Internal.into(), Some("OTA: ESP_FAIL")).as_deref(),
            Some("OTA: ESP_FAIL")
        );
        assert_eq!(
            update_failure(ErrorCode::Internal.into(), Some("DHT11: timeout")),
            None
        );
    }

    #[tokio::test]
    async fn only_update_errors_fail_an_update() {
        let hub = Hub::for_tests("update-fails", "").await;
        on_message(&hub, &hello("board")).await.unwrap();
        hub.ota_for_tests()
            .start("board", "0.9.0".into(), "0.9.1".into(), "0.9.1.bin".into())
            .unwrap();
        let error = |code: ErrorCode, detail: &str| {
            Message::new(
                "board",
                1,
                Payload::Error {
                    code: code.into(),
                    detail: Some(detail.into()),
                },
            )
        };
        let titles = |since| -> Vec<String> {
            hub.notifications_since(since)
                .into_iter()
                .map(|notification| notification.title)
                .collect()
        };

        let before = hub.last_notification_id();
        on_message(&hub, &error(ErrorCode::Internal, "DHT11: timeout"))
            .await
            .unwrap();
        assert!(hub.is_updating("board"));

        on_message(
            &hub,
            &error(ErrorCode::UpdateRejected, "ESP_ERR_OTA_VALIDATE_FAILED"),
        )
        .await
        .unwrap();
        assert!(!hub.is_updating("board"));
        assert!(titles(before).contains(&"board update failed".to_string()));
    }

    #[tokio::test]
    async fn answers_and_presence() {
        let hub = Hub::for_tests("answers", "").await;
        on_message(&hub, &hello("lamp")).await.unwrap();
        on_message(&hub, &hello("other")).await.unwrap();

        let mut sent = hub.traffic().subscribe();
        let command = tokio::spawn({
            let hub = hub.clone();
            async move {
                hub.set("lamp", 1, 0, Value::Bool(true), CommandOrigin::Web)
                    .await
            }
        });
        let message_id = Message::from_json(&sent.recv().await.unwrap().text)
            .unwrap()
            .msg_id;

        on_message(&hub, &ack("other", message_id)).await.unwrap();
        tokio::task::yield_now().await;
        assert!(!command.is_finished());
        assert_eq!(hub.value("other", 1, 0), None);
        assert_eq!(hub.value("lamp", 1, 0), None);

        on_message(&hub, &ack("lamp", message_id)).await.unwrap();
        command.await.unwrap().unwrap();
        assert_eq!(hub.value("lamp", 1, 0), Some(Value::Bool(true)));

        hub.check_presence(Instant::now() + Duration::from_secs(3600));
        let before = hub.last_notification_id();
        on_message(&hub, &hello("lamp")).await.unwrap();
        let titles: Vec<String> = hub
            .notifications_since(before)
            .into_iter()
            .map(|notification| notification.title)
            .collect();
        assert_eq!(titles, vec!["lamp back online"]);
        assert!(hub.is_online("lamp"));
    }
}
