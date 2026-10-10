//! Talks to an Oxihome node over serial or MQTT: prints what nodes send and sends commands typed on stdin.

use std::io::{self, BufRead, BufReader, Write};
use std::thread;
use std::time::Duration;

use anyhow::{Context, bail};
use oxihome_protocol::{CommandOp, Message, Payload, QueryKind, Value};
use rumqttc::{Client, Event, MqttOptions, Packet, QoS};
use serialport::SerialPort;

const BAUD_RATE: u32 = 115_200;
const DEFAULT_MQTT_PORT: u16 = 1883;
const SOURCE: &str = "oxihome-client";

const USAGE: &str = "\
usage:
  oxihome-client serial <port>                  e.g. serial /dev/ttyACM0
  oxihome-client mqtt <host[:port]> <device_id> e.g. mqtt localhost esp32-08d1f9ca41e4";

const HELP: &str = "\
commands:
  set <entity> <channel> <true|false|number>
  query hello
  query state [entity]
  help
  quit";

enum Link {
    Serial(Box<dyn SerialPort>),
    Mqtt { client: Client, down_topic: String },
}

impl Link {
    fn send(&mut self, message: &Message) -> anyhow::Result<()> {
        let json = message.to_json()?;
        match self {
            Link::Serial(port) => port.write_all(format!("{json}\n").as_bytes())?,
            Link::Mqtt { client, down_topic } => {
                client.publish(down_topic.as_str(), QoS::AtLeastOnce, false, json)?
            }
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let (mut link, destination) = match arguments.as_slice() {
        ["serial", path] => (open_serial(path)?, None),
        ["mqtt", address, device_id] => {
            (open_mqtt(address, device_id)?, Some(device_id.to_string()))
        }
        _ => bail!("{USAGE}"),
    };

    println!("{HELP}");
    let mut next_message_id = 0u32;

    for line in io::stdin().lock().lines() {
        let line = line?;
        let words: Vec<&str> = line.split_whitespace().collect();
        let payload = match words.as_slice() {
            [] => continue,
            ["quit"] => break,
            ["help"] => {
                println!("{HELP}");
                continue;
            }
            command => match parse_command(command) {
                Ok(payload) => payload,
                Err(error) => {
                    println!("error: {error}");
                    continue;
                }
            },
        };

        let message = Message {
            version: 1,
            msg_id: next_message_id,
            src: format!("{SOURCE}-{}", std::process::id()),
            dst: destination.clone(),
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload,
            auth: None,
        };
        next_message_id = next_message_id.wrapping_add(1);

        println!("-> #{} {:?}", message.msg_id, message.payload);
        link.send(&message)?;
    }
    Ok(())
}

fn open_serial(path: &str) -> anyhow::Result<Link> {
    let mut port = serialport::new(path, BAUD_RATE)
        .timeout(Duration::from_millis(100))
        .open()
        .with_context(|| format!("opening {path}"))?;
    // Keep the board's auto-reset circuit (EN / GPIO0) released.
    port.write_data_terminal_ready(false)?;
    port.write_request_to_send(false)?;

    let reader = port.try_clone()?;
    thread::spawn(move || read_serial(reader));
    println!("connected to {path}");
    Ok(Link::Serial(port))
}

fn open_mqtt(address: &str, device_id: &str) -> anyhow::Result<Link> {
    let (host, port) = match address.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().context("MQTT port")?),
        None => (address, DEFAULT_MQTT_PORT),
    };
    let host = host.to_string();
    let options = MqttOptions::new(
        format!("{SOURCE}-{}", std::process::id()),
        host.clone(),
        port,
    );
    let (client, mut connection) = Client::new(options, 16);

    let subscriber = client.clone();
    thread::spawn(move || {
        for event in connection.iter() {
            match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => {
                    println!("connected to {host}:{port}");
                    if let Err(error) = subscriber.subscribe("oxihome/+/up", QoS::AtLeastOnce) {
                        eprintln!("subscribe failed: {error}");
                    }
                }
                Ok(Event::Incoming(Packet::Publish(publish))) => {
                    print_line(&String::from_utf8_lossy(&publish.payload));
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("MQTT connection error: {error}, retrying");
                    thread::sleep(Duration::from_secs(1));
                }
            }
        }
    });

    Ok(Link::Mqtt {
        client,
        down_topic: format!("oxihome/{device_id}/down"),
    })
}

fn parse_command(words: &[&str]) -> anyhow::Result<Payload> {
    Ok(match words {
        ["set", entity, channel, value] => Payload::Command {
            entity: entity.parse().context("entity")?,
            channel: channel.parse().context("channel")?,
            op: CommandOp::Set(parse_value(value)?),
        },
        ["query", "hello"] => Payload::Query {
            what: QueryKind::Hello,
        },
        ["query", "state"] => Payload::Query {
            what: QueryKind::State { entity: None },
        },
        ["query", "state", entity] => Payload::Query {
            what: QueryKind::State {
                entity: Some(entity.parse().context("entity")?),
            },
        },
        _ => bail!("unknown command, type `help`"),
    })
}

fn parse_value(text: &str) -> anyhow::Result<Value> {
    if let Ok(value) = text.parse::<bool>() {
        Ok(Value::Bool(value))
    } else if let Ok(value) = text.parse::<i32>() {
        Ok(Value::Int(value))
    } else if let Ok(value) = text.parse::<f32>() {
        Ok(Value::Float(value))
    } else {
        bail!("value must be true, false or a number")
    }
}

fn read_serial(port: Box<dyn SerialPort>) {
    let mut reader = BufReader::new(port);
    // Raw bytes: the node prints non-UTF-8 noise while it resets.
    let mut line = Vec::new();
    loop {
        match reader.read_until(b'\n', &mut line) {
            Ok(_) if line.ends_with(b"\n") => {}
            Ok(_) => continue,
            // A timeout keeps the partial line in `line`; the next read appends to it.
            Err(error) if error.kind() == io::ErrorKind::TimedOut => continue,
            Err(error) => {
                eprintln!("serial read failed: {error}");
                return;
            }
        }

        print_line(String::from_utf8_lossy(&line).trim_end());
        line.clear();
    }
}

/// Lines that are not protocol messages are the node's log output.
fn print_line(text: &str) {
    match Message::from_json(text) {
        Ok(message) => {
            let reply = message
                .reply_to
                .map(|id| format!(" (reply to #{id})"))
                .unwrap_or_default();
            println!(
                "<- {} #{}{reply} {:?}",
                message.src, message.msg_id, message.payload
            );
        }
        Err(_) => println!("   {text}"),
    }
}
