//! Talks to an Oxihome node over serial: prints what the node sends and sends commands typed on stdin.

use std::io::{self, BufRead, BufReader, Write};
use std::thread;
use std::time::Duration;

use anyhow::{Context, bail};
use oxihome_protocol::{CommandOp, Message, Payload, QueryKind, Value};
use serialport::SerialPort;

const DEFAULT_PORT: &str = "/dev/ttyACM0";
const BAUD_RATE: u32 = 115_200;
const SOURCE: &str = "serial-client";

const HELP: &str = "\
commands:
  set <entity> <channel> <true|false|number>
  query hello
  query state [entity]
  help
  quit";

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_PORT.into());

    let mut port = serialport::new(&path, BAUD_RATE)
        .timeout(Duration::from_secs(1))
        .open()
        .with_context(|| format!("opening {path}"))?;
    // Keep the board's auto-reset circuit (EN / GPIO0) released.
    port.write_data_terminal_ready(false)?;
    port.write_request_to_send(false)?;

    let reader = port.try_clone()?;
    thread::spawn(move || print_incoming(reader));

    println!("connected to {path}\n{HELP}");
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
            src: SOURCE.into(),
            dst: None,
            ttl: 1,
            reply_to: None,
            timestamp: None,
            payload,
            auth: None,
        };
        next_message_id = next_message_id.wrapping_add(1);

        println!("-> #{} {:?}", message.msg_id, message.payload);
        let mut json = message.to_json()?;
        json.push('\n');
        port.write_all(json.as_bytes())?;
    }
    Ok(())
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

/// Lines that are not protocol messages are the node's log output.
fn print_incoming(port: Box<dyn SerialPort>) {
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

        let text = String::from_utf8_lossy(&line).trim_end().to_string();
        line.clear();
        match Message::from_json(&text) {
            Ok(message) => {
                let reply = message
                    .reply_to
                    .map(|id| format!(" (reply to #{id})"))
                    .unwrap_or_default();
                println!("<- #{}{reply} {:?}", message.msg_id, message.payload);
            }
            Err(_) => println!("   {text}"),
        }
    }
}
