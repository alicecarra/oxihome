//! Runs a software node over MQTT: one connection per node, like a real board.

use std::time::Duration;

use oxihome_protocol::Message;
use rumqttc::{AsyncClient, Event, MqttOptions, Packet, QoS};
use tokio::sync::mpsc;

const BROADCAST_TOPIC: &str = "oxihome/broadcast/down";

/// A node that runs on a computer: the protocol side usually comes from a [`crate::NodeCore`].
pub trait SoftwareNode: Send + 'static {
    fn device_id(&self) -> &str;

    /// The answers to a message from the hub.
    fn handle(&mut self, message: &Message) -> Vec<Message>;

    /// Sent after every connection: the `Hello` and the states.
    fn announcement(&mut self) -> Vec<Message>;

    /// Sent every report interval, usually a heartbeat and the sensor states.
    fn report(&mut self) -> Vec<Message>;
}

pub struct Broker {
    pub host: String,
    pub port: u16,
    /// How often [`SoftwareNode::report`] runs; heartbeats should match the node's
    /// `HeartbeatInterval` (30 s by default).
    pub report_interval: Duration,
}

/// Changes a node from outside the MQTT loop, e.g. a console or a chat bot.
pub struct Handle<N> {
    sender: mpsc::Sender<Control<N>>,
}

impl<N> Clone for Handle<N> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
        }
    }
}

type Action<N> = Box<dyn FnOnce(&mut N) -> Vec<Message> + Send>;

enum Control<N> {
    /// Runs on the node; the messages it returns are published.
    Act(Action<N>),
    /// Disconnects and stays away until `Connect`, like a board losing power.
    Disconnect,
    Connect,
}

impl<N> Handle<N> {
    /// Runs `action` on the node and publishes what it returns. `false` when the node stopped.
    pub async fn act(&self, action: impl FnOnce(&mut N) -> Vec<Message> + Send + 'static) -> bool {
        self.sender
            .send(Control::Act(Box::new(action)))
            .await
            .is_ok()
    }

    /// `false` when the node stopped, here and in `connect`.
    pub async fn disconnect(&self) -> bool {
        self.sender.send(Control::Disconnect).await.is_ok()
    }

    pub async fn connect(&self) -> bool {
        self.sender.send(Control::Connect).await.is_ok()
    }
}

/// Spawns the node's MQTT loop on the current Tokio runtime and returns its handle. The loop
/// reconnects by itself and ends when every handle is dropped.
pub fn spawn<N: SoftwareNode>(node: N, broker: Broker) -> Handle<N> {
    let (sender, receiver) = mpsc::channel(16);
    tokio::spawn(run(node, broker, receiver));
    Handle { sender }
}

async fn run<N: SoftwareNode>(
    mut node: N,
    broker: Broker,
    mut controls: mpsc::Receiver<Control<N>>,
) {
    let device_id = node.device_id().to_string();
    let up_topic = format!("oxihome/{device_id}/up");
    let down_topic = format!("oxihome/{device_id}/down");

    loop {
        let options = MqttOptions::new(device_id.clone(), broker.host.clone(), broker.port);
        let (client, mut event_loop) = AsyncClient::new(options, 32);
        let mut report = tokio::time::interval(broker.report_interval);
        report.tick().await;

        let disconnected = loop {
            let outgoing = tokio::select! {
                event = event_loop.poll() => match event {
                    Ok(Event::Incoming(Packet::ConnAck(_))) => {
                        println!("{device_id} connected");
                        for topic in [down_topic.as_str(), BROADCAST_TOPIC] {
                            if let Err(error) = client.subscribe(topic, QoS::AtLeastOnce).await {
                                println!("{device_id} subscribe failed: {error}");
                            }
                        }
                        node.announcement()
                    }
                    Ok(Event::Incoming(Packet::Publish(publish))) => {
                        match Message::from_json(&String::from_utf8_lossy(&publish.payload)) {
                            Ok(message) => node.handle(&message),
                            Err(error) => {
                                println!("{device_id} invalid message: {error}");
                                Vec::new()
                            }
                        }
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => {
                        println!("{device_id} connection error: {error}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        Vec::new()
                    }
                },
                _ = report.tick() => node.report(),
                control = controls.recv() => match control {
                    Some(Control::Act(action)) => action(&mut node),
                    Some(Control::Disconnect) => {
                        println!("{device_id} going offline");
                        let _ = client.disconnect().await;
                        break true;
                    }
                    Some(Control::Connect) => Vec::new(),
                    None => break false,
                },
            };
            publish(&client, &device_id, &up_topic, outgoing).await;
        };
        if !disconnected || !wait_for_connect(&mut node, &device_id, &mut controls).await {
            return;
        }
    }
}

/// While offline, actions still change the node but nothing is sent. `false` when the node
/// stopped running.
async fn wait_for_connect<N>(
    node: &mut N,
    device_id: &str,
    controls: &mut mpsc::Receiver<Control<N>>,
) -> bool {
    loop {
        match controls.recv().await {
            Some(Control::Connect) => return true,
            Some(Control::Act(action)) => {
                if !action(node).is_empty() {
                    println!("{device_id} is offline, nothing sent");
                }
            }
            Some(Control::Disconnect) => println!("{device_id} is already offline"),
            None => return false,
        }
    }
}

async fn publish(client: &AsyncClient, device_id: &str, topic: &str, messages: Vec<Message>) {
    for message in messages {
        let json = match message.to_json() {
            Ok(json) => json,
            Err(error) => {
                println!("{device_id} encoding failed: {error}");
                continue;
            }
        };
        if let Err(error) = client.publish(topic, QoS::AtLeastOnce, false, json).await {
            println!("{device_id} publish failed: {error}");
        }
    }
}
