//! A software node with the state of a Creality K1 family 3D printer, from its local websocket.

mod creality;
mod node;

use std::time::Duration;

use oxihome_node::config::{BrokerConfig, read_config};
use oxihome_node::mqtt::{self, Handle};
use serde::Deserialize;

use node::PrinterNode;

const RECONNECT_DELAY: Duration = Duration::from_secs(10);

#[derive(Deserialize)]
struct Config {
    mqtt: BrokerConfig,
    node: NodeConfig,
    printer: PrinterConfig,
}

#[derive(Deserialize)]
struct NodeConfig {
    id: String,
}

#[derive(Deserialize)]
struct PrinterConfig {
    host: String,
    #[serde(default = "default_port")]
    port: u16,
}

fn default_port() -> u16 {
    9999
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let (_, config): (String, Config) = read_config("oxihome-printer <printer.toml>")?;

    let handle = mqtt::spawn(
        PrinterNode::new(config.node.id.clone()),
        config.mqtt.broker(),
    );
    let url = format!("ws://{}:{}/", config.printer.host, config.printer.port);
    // The printer is often off: log a failure once, not every attempt.
    let mut last_failure = None;
    loop {
        let outcome = tokio::select! {
            outcome = follow(&url, &handle, &mut last_failure) => outcome,
            () = oxihome_node::shutdown_signal() => return Ok(()),
        };
        match outcome {
            Ok(()) => return Ok(()),
            Err(error) => {
                let failure = format!("{error:#}");
                if last_failure.as_ref() != Some(&failure) {
                    println!("printer at {url}: {failure}");
                    last_failure = Some(failure);
                }
            }
        }
        if !handle.act(|node| node.disconnected()).await {
            return Ok(());
        }
        tokio::select! {
            () = tokio::time::sleep(RECONNECT_DELAY) => {}
            () = oxihome_node::shutdown_signal() => return Ok(()),
        }
    }
}

/// `Ok` only when the node stopped.
async fn follow(
    url: &str,
    handle: &Handle<PrinterNode>,
    last_failure: &mut Option<String>,
) -> anyhow::Result<()> {
    let mut connection = creality::Connection::open(url).await?;
    println!("connected to {url}");
    *last_failure = None;
    loop {
        let update = connection.next().await?;
        if !handle.act(move |node| node.update(update)).await {
            return Ok(());
        }
    }
}
