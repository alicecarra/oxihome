//! The websocket of the stock firmware (port 9999): a whole status object when it connects, then
//! only the fields that changed, a few times a second while printing.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// The printer's own web page sends one every 5 s.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
/// It answers every heartbeat, so this long without a message means it is gone.
const SILENCE_LIMIT: Duration = Duration::from_secs(30);
/// The filament box is only sent when asked: on connecting, this often, and when the printer says
/// the feeding changed.
const BOX_INTERVAL: Duration = Duration::from_secs(60);
/// Fields that change when a filament is loaded, unloaded or swapped.
const FEEDING_FIELDS: [&str; 3] = ["feedState", "materialStatus", "cfsConnect"];

pub struct Connection {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    heartbeat: tokio::time::Interval,
    boxes: tokio::time::Interval,
}

impl Connection {
    pub async fn open(url: &str) -> anyhow::Result<Self> {
        let (socket, _) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(url))
                .await
                .context("timed out connecting")??;
        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut boxes = tokio::time::interval(BOX_INTERVAL);
        boxes.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        Ok(Self {
            socket,
            heartbeat,
            boxes,
        })
    }

    /// The next status update; other messages (the answers to the heartbeat) are skipped.
    pub async fn next(&mut self) -> anyhow::Result<Map<String, Value>> {
        loop {
            let message = tokio::select! {
                _ = self.heartbeat.tick() => {
                    self.socket.send(Message::text(heartbeat())).await?;
                    continue;
                }
                _ = self.boxes.tick() => {
                    self.socket.send(Message::text(BOX_REQUEST)).await?;
                    continue;
                }
                message = tokio::time::timeout(SILENCE_LIMIT, self.socket.next()) => message,
            };
            let text = match message.context("silent for too long")? {
                Some(Ok(Message::Text(text))) => text,
                Some(Ok(Message::Close(_))) | None => bail!("closed"),
                Some(Ok(_)) => continue,
                Some(Err(error)) => return Err(error.into()),
            };
            if let Ok(Value::Object(update)) = serde_json::from_str::<Value>(&text) {
                if FEEDING_FIELDS
                    .iter()
                    .any(|field| update.contains_key(*field))
                {
                    self.boxes.reset_immediately();
                }
                return Ok(update);
            }
        }
    }
}

const BOX_REQUEST: &str = r#"{"method":"get","params":{"boxsInfo":1}}"#;

fn heartbeat() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    serde_json::json!({ "ModeCode": "heart_beat", "msg": now.to_string() }).to_string()
}

/// Every field received so far, each with its last value.
#[derive(Default)]
pub struct Status {
    fields: Map<String, Value>,
}

impl Status {
    pub fn merge(&mut self, update: Map<String, Value>) {
        self.fields.extend(update);
    }

    /// Temperatures come as strings ("220.480000"), the rest as numbers.
    pub fn number(&self, key: &str) -> Option<f64> {
        match self.fields.get(key)? {
            Value::Number(number) => number.as_f64(),
            Value::String(text) => text.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn text(&self, key: &str) -> Option<&str> {
        self.fields.get(key)?.as_str()
    }

    pub fn error_code(&self) -> Option<f64> {
        self.fields.get("err")?.get("errcode")?.as_f64()
    }

    pub fn job_state(&self) -> Option<JobState> {
        JobState::from_code(self.number("state")?)
    }

    /// The first filament box (`type` 0; the spool holder outside is `type` 1), from the answer
    /// to the box request.
    pub fn material_box(&self) -> Option<MaterialBox> {
        let found = self
            .fields
            .get("boxsInfo")?
            .get("materialBoxs")?
            .as_array()?
            .iter()
            .find(|found| found.get("type").and_then(Value::as_i64) == Some(0))?;
        let slots = found
            .get("materials")
            .and_then(Value::as_array)
            .map(|materials| materials.iter().filter_map(Slot::from_json).collect())
            .unwrap_or_default();
        Some(MaterialBox {
            temperature: found.get("temp").and_then(Value::as_f64),
            humidity: found.get("humidity").and_then(Value::as_f64),
            slots,
        })
    }
}

pub struct MaterialBox {
    pub temperature: Option<f64>,
    pub humidity: Option<f64>,
    pub slots: Vec<Slot>,
}

pub struct Slot {
    /// 1 to 4; the printer counts from 0.
    pub number: u8,
    pub color: Option<(u8, u8, u8)>,
    pub material: String,
    /// Percent.
    pub left: i32,
    /// Feeding the printer.
    pub selected: bool,
}

impl Slot {
    fn from_json(material: &Value) -> Option<Self> {
        let id = material.get("id")?.as_u64()?;
        Some(Slot {
            number: u8::try_from(id + 1).ok()?,
            color: material
                .get("color")
                .and_then(Value::as_str)
                .and_then(color),
            material: material
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            left: material
                .get("percent")
                .and_then(Value::as_i64)
                .map_or(0, |percent| percent.clamp(0, 100) as i32),
            selected: material.get("selected").and_then(Value::as_i64) == Some(1),
        })
    }
}

/// "#0b2a1e1": the last six digits are the red, green and blue.
fn color(text: &str) -> Option<(u8, u8, u8)> {
    let digits = text.strip_prefix('#')?;
    let rgb = digits.get(digits.len().checked_sub(6)?..)?;
    let channel = |start: usize| u8::from_str_radix(rgb.get(start..start + 2)?, 16).ok();
    Some((channel(0)?, channel(2)?, channel(4)?))
}

/// The codes and order of the `state` field, from the printer's web page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    Idle,
    Printing,
    Finished,
    Failed,
    Cancelled,
    Paused,
}

impl JobState {
    pub const NAMES: [&str; 6] = [
        "idle",
        "printing",
        "finished",
        "failed",
        "cancelled",
        "paused",
    ];

    fn from_code(code: f64) -> Option<Self> {
        Some(match code as i64 {
            0 => Self::Idle,
            1 => Self::Printing,
            2 => Self::Finished,
            3 => Self::Failed,
            4 => Self::Cancelled,
            5 => Self::Paused,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        Self::NAMES[self as usize]
    }
}
