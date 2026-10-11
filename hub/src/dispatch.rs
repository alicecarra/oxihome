//! What the hub does with each thing that happens: a broker connection, a message, a presence check, a clock tick.

use std::sync::Arc;
use std::time::Instant;

use jiff::Timestamp;
use oxihome_protocol::{Message, Payload, QueryKind, management};

use crate::config::rules;
use crate::hub::Hub;
use crate::node::parameters;
use crate::node::registry::NodeEvent;
use crate::schedule::Schedule;

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

pub async fn on_message(hub: &Arc<Hub>, message: &Message) -> anyhow::Result<()> {
    let (node_events, updates, event) = hub.handle(message, Instant::now());

    let reports_parameters = matches!(
        message.payload,
        Payload::State {
            entity: management::ENTITY,
            ..
        }
    );
    let came_back = node_events
        .iter()
        .any(|node_event| matches!(node_event, NodeEvent::Online(_)));
    if reports_parameters || came_back {
        tokio::spawn(parameters::apply(hub.clone(), message.src.clone()));
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
    }

    if let Some(event) = event {
        println!("{event}");
        for rule in hub.matching_rules(&event) {
            tokio::spawn(rules::run(hub.clone(), rule));
        }
    }
    Ok(())
}

/// Marks nodes that went quiet as offline.
pub fn on_presence_check(hub: &Hub) {
    for node_event in hub.check_presence(Instant::now()) {
        on_node_event(hub, &node_event);
    }
}

/// Runs the scheduled rules of each new local minute.
pub fn on_clock_tick(hub: &Arc<Hub>, schedule: &mut Schedule) {
    for minute in schedule.new_minutes(Timestamp::now()) {
        for rule in hub.due_rules(minute) {
            tokio::spawn(rules::run(hub.clone(), rule));
        }
    }
}

fn on_node_event(hub: &Hub, node_event: &NodeEvent) {
    println!("{node_event}");
    hub.notify_node_event(node_event);
}
