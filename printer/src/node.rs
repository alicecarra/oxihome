use std::collections::HashMap;
use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode,
    EventKind, Message, Quantity, Unit, Value, ValueType,
};
use serde_json::Map;

use crate::creality::{JobState, Status};

const TEMPERATURES_ENTITY: u8 = 1;
const JOB_ENTITY: u8 = 2;
const PRINTER_ENTITY: u8 = 3;
const CONNECTION_ENTITY: u8 = 4;
const ENTITIES: [u8; 4] = [
    TEMPERATURES_ENTITY,
    JOB_ENTITY,
    PRINTER_ENTITY,
    CONNECTION_ENTITY,
];

const JOB_ELAPSED_CHANNEL: u8 = 4;
const JOB_LEFT_CHANNEL: u8 = 5;
const JOB_CHANGE_CHANNEL: u8 = 7;
const FILE_MAX_LENGTH: u16 = 128;

/// The printer sends a few updates a second; between reports only changes this big go out.
const TEMPERATURE_STEP: f32 = 0.5;
const TIME_STEP_S: i32 = 60;

pub struct PrinterNode {
    core: NodeCore,
    device: PrinterDevice,
    /// What the hub last got of each entity.
    published: HashMap<u8, Vec<ChannelValue>>,
    started_at: Instant,
}

struct PrinterDevice {
    status: Option<Status>,
    /// `None` until the first connection works or fails.
    reachable: Option<bool>,
}

impl PrinterNode {
    pub fn new(device_id: String) -> Self {
        Self {
            core: NodeCore::new(
                device_id,
                Build::native("printer", env!("CARGO_PKG_VERSION")),
            ),
            device: PrinterDevice {
                status: None,
                reachable: None,
            },
            published: HashMap::new(),
            started_at: Instant::now(),
        }
    }

    pub fn update(&mut self, update: Map<String, serde_json::Value>) -> Vec<Message> {
        let status = self.device.status.get_or_insert_with(Status::default);
        let before = status.job_state();
        let first = before.is_none();
        status.merge(update);
        let after = status.job_state();
        self.device.reachable = Some(true);

        let mut messages = self.changes(significant);
        if let Some(after) = after
            && before != Some(after)
        {
            println!(
                "job: {} → {}",
                before.map_or("unknown", JobState::name),
                after.name()
            );
            if !first {
                messages.push(self.core.event(
                    JOB_ENTITY,
                    JOB_CHANGE_CHANNEL,
                    EventKind::Received,
                    Some(Value::Text(after.name().into())),
                ));
            }
        }
        messages
    }

    /// The readings keep their last values; the connection entity tells they are old.
    pub fn disconnected(&mut self) -> Vec<Message> {
        self.device.reachable = Some(false);
        self.changes(|_, old, new| old != new)
    }

    /// The entities whose values changed enough since the hub last got them.
    fn changes(
        &mut self,
        enough: fn(u8, &[ChannelValue], &[ChannelValue]) -> bool,
    ) -> Vec<Message> {
        let mut messages = Vec::new();
        for entity in ENTITIES {
            let Some(values) = self.device.state(entity) else {
                continue;
            };
            let changed = self
                .published
                .get(&entity)
                .is_none_or(|old| enough(entity, old, &values));
            if changed {
                messages.extend(self.core.state(&self.device, entity));
                self.published.insert(entity, values);
            }
        }
        messages
    }
}

fn significant(entity: u8, old: &[ChannelValue], new: &[ChannelValue]) -> bool {
    if old.len() != new.len() {
        return true;
    }
    old.iter().zip(new).any(|(old, new)| {
        if old.channel != new.channel {
            return true;
        }
        match (entity, new.channel, &old.value, &new.value) {
            (TEMPERATURES_ENTITY, _, Value::Float(old), Value::Float(new)) => {
                (old - new).abs() >= TEMPERATURE_STEP
            }
            (
                JOB_ENTITY,
                JOB_ELAPSED_CHANNEL | JOB_LEFT_CHANNEL,
                Value::Int(old),
                Value::Int(new),
            ) => (old - new).abs() >= TIME_STEP_S,
            (_, _, old, new) => old != new,
        }
    })
}

impl SoftwareNode for PrinterNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    fn invalid_message(&mut self, detail: String) -> Vec<Message> {
        vec![self.core.invalid_message(detail)]
    }

    fn announcement(&mut self) -> Vec<Message> {
        self.published = ENTITIES
            .into_iter()
            .filter_map(|entity| Some((entity, self.device.state(entity)?)))
            .collect();
        self.core.announcement(&self.device)
    }

    /// Also sends the small changes held back since the last report.
    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        let mut messages = vec![self.core.heartbeat(uptime_s, None, None)];
        messages.extend(self.changes(|_, old, new| old != new));
        messages
    }
}

impl Device for PrinterDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let temperature =
            |id, max, name| reading(id, 0.0, max, Quantity::Temperature, Unit::Celsius).named(name);
        vec![
            EntityDescriptor {
                id: TEMPERATURES_ENTITY,
                name: None,
                channels: vec![
                    temperature(0, 350.0, "Nozzle"),
                    temperature(1, 350.0, "Nozzle target"),
                    temperature(2, 150.0, "Bed"),
                    temperature(3, 150.0, "Bed target"),
                    temperature(4, 100.0, "Chamber"),
                ],
            }
            .named("Temperatures"),
            EntityDescriptor {
                id: JOB_ENTITY,
                name: None,
                channels: vec![
                    read(
                        0,
                        ValueType::Enum(JobState::NAMES.map(String::from).to_vec()),
                        None,
                    )
                    .named("State"),
                    read(1, integer(100), Some(Unit::Percent)).named("Progress"),
                    read(2, integer(i32::MAX), None).named("Layer"),
                    read(3, integer(i32::MAX), None).named("Layers"),
                    read(JOB_ELAPSED_CHANNEL, integer(i32::MAX), Some(Unit::Second))
                        .named("Elapsed"),
                    read(JOB_LEFT_CHANNEL, integer(i32::MAX), Some(Unit::Second)).named("Left"),
                    read(
                        6,
                        ValueType::Text {
                            max_length: FILE_MAX_LENGTH,
                        },
                        None,
                    )
                    .named("File"),
                    Channel {
                        id: JOB_CHANGE_CHANNEL,
                        name: None,
                        access: Access::Event,
                        value: Some(ValueType::Text { max_length: 16 }),
                        quantity: None,
                        unit: None,
                    }
                    .named("Change"),
                ],
            }
            .named("Job"),
            EntityDescriptor {
                id: PRINTER_ENTITY,
                name: None,
                channels: vec![
                    read(0, ValueType::Bool, None).named("Light"),
                    read(1, integer(i32::MAX), None).named("Error code"),
                ],
            }
            .named("Printer"),
            EntityDescriptor {
                id: CONNECTION_ENTITY,
                name: None,
                channels: vec![read(0, ValueType::Bool, None).named("Reachable")],
            }
            .named("Connection"),
        ]
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        if entity == CONNECTION_ENTITY {
            return Some(vec![confirmed(0, Value::Bool(self.reachable?))]);
        }
        let status = self.status.as_ref()?;
        let float = |channel, key| {
            let value = status.number(key)?;
            Some(confirmed(channel, Value::Float(value as f32)))
        };
        let int = |channel, key| {
            let value = status.number(key)?;
            Some(confirmed(channel, Value::Int(value as i32)))
        };
        let values: Vec<ChannelValue> = match entity {
            TEMPERATURES_ENTITY => [
                float(0, "nozzleTemp"),
                float(1, "targetNozzleTemp"),
                float(2, "bedTemp0"),
                float(3, "targetBedTemp0"),
                float(4, "boxTemp"),
            ]
            .into_iter()
            .flatten()
            .collect(),
            JOB_ENTITY => [
                status
                    .job_state()
                    .map(|state| confirmed(0, Value::Enum(state.name().into()))),
                int(1, "printProgress"),
                int(2, "layer"),
                int(3, "TotalLayer"),
                int(JOB_ELAPSED_CHANNEL, "printJobTime"),
                int(JOB_LEFT_CHANNEL, "printLeftTime"),
                status
                    .text("printFileName")
                    .map(|path| confirmed(6, Value::Text(file_name(path)))),
            ]
            .into_iter()
            .flatten()
            .collect(),
            PRINTER_ENTITY => [
                status
                    .number("lightSw")
                    .map(|light| confirmed(0, Value::Bool(light != 0.0))),
                status
                    .error_code()
                    .map(|code| confirmed(1, Value::Int(code as i32))),
            ]
            .into_iter()
            .flatten()
            .collect(),
            _ => return None,
        };
        (!values.is_empty()).then_some(values)
    }

    /// Every channel is read only, so the core refuses commands before they get here.
    fn command(&mut self, entity: u8, channel: u8, _op: &CommandOp) -> Result<(), Rejection> {
        Err(Rejection::new(
            ErrorCode::InvalidOperation,
            format!("entity {entity} channel {channel} is read only"),
        ))
    }
}

/// "/usr/data/printer_data/gcodes/Box.stl_PLA_1h2m3s.gcode" → "Box.stl_PLA_1h2m3s".
fn file_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.strip_suffix(".gcode").unwrap_or(name);
    let mut end = name.len().min(usize::from(FILE_MAX_LENGTH));
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_string()
}

fn reading(id: u8, min: f32, max: f32, quantity: Quantity, unit: Unit) -> Channel {
    Channel {
        id,
        name: None,
        access: Access::Read,
        value: Some(ValueType::Float {
            min,
            max,
            step: 0.1,
        }),
        quantity: Some(quantity),
        unit: Some(unit),
    }
}

fn read(id: u8, value: ValueType, unit: Option<Unit>) -> Channel {
    Channel {
        id,
        name: None,
        access: Access::Read,
        value: Some(value),
        quantity: None,
        unit,
    }
}

fn integer(max: i32) -> ValueType {
    ValueType::Int {
        min: 0,
        max,
        step: 1,
    }
}

fn confirmed(channel: u8, value: Value) -> ChannelValue {
    ChannelValue {
        channel,
        value,
        confidence: Confidence::Confirmed,
    }
}

#[cfg(test)]
mod tests {
    use oxihome_protocol::Payload;
    use serde_json::json;

    use super::*;

    fn update(value: serde_json::Value) -> Map<String, serde_json::Value> {
        value.as_object().unwrap().clone()
    }

    fn printing() -> Map<String, serde_json::Value> {
        update(json!({
            "nozzleTemp": "220.480000", "targetNozzleTemp": 220,
            "bedTemp0": "60.200000", "targetBedTemp0": 60, "boxTemp": 33,
            "state": 1, "printProgress": 48, "layer": 256, "TotalLayer": 525,
            "printJobTime": 5130, "printLeftTime": 7464,
            "printFileName": "/usr/data/printer_data/gcodes/Shell.stl_PLA_3h31m11s.gcode",
            "lightSw": 1, "err": { "errcode": 0, "key": 0 },
        }))
    }

    fn states(messages: &[Message]) -> Vec<u8> {
        messages
            .iter()
            .filter_map(|message| match message.payload {
                Payload::State { entity, .. } => Some(entity),
                _ => None,
            })
            .collect()
    }

    fn events(messages: &[Message]) -> Vec<Value> {
        messages
            .iter()
            .filter_map(|message| match &message.payload {
                Payload::Event { data, .. } => data.clone(),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_first_status_publishes_every_entity_without_an_event() {
        let mut node = PrinterNode::new("printer".into());
        let messages = node.update(printing());
        assert_eq!(states(&messages), ENTITIES);
        assert!(events(&messages).is_empty());
        let job = node.device.state(JOB_ENTITY).unwrap();
        assert_eq!(job[0].value, Value::Enum("printing".into()));
        assert_eq!(job[6].value, Value::Text("Shell.stl_PLA_3h31m11s".into()));
        assert_eq!(
            node.device.state(TEMPERATURES_ENTITY).unwrap()[0].value,
            Value::Float(220.48)
        );
    }

    #[test]
    fn small_changes_wait_for_the_report() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        let small = node.update(update(
            json!({ "nozzleTemp": "220.700000", "printJobTime": 5140 }),
        ));
        assert!(states(&small).is_empty());
        let big = node.update(update(json!({ "nozzleTemp": "221.100000" })));
        assert_eq!(states(&big), [TEMPERATURES_ENTITY]);
        let report = node.report();
        assert_eq!(states(&report), [JOB_ENTITY], "the elapsed time held back");
        assert!(states(&node.report()).is_empty());
    }

    #[test]
    fn a_new_job_state_is_an_event() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        let messages = node.update(update(json!({ "state": 2, "printProgress": 100 })));
        assert_eq!(events(&messages), [Value::Text("finished".into())]);
        assert!(events(&node.update(update(json!({ "state": 2 })))).is_empty());
    }

    #[test]
    fn losing_the_printer_keeps_the_readings() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        assert_eq!(states(&node.disconnected()), [CONNECTION_ENTITY]);
        assert!(node.disconnected().is_empty());
        assert_eq!(
            node.device.state(JOB_ENTITY).unwrap()[1].value,
            Value::Int(48)
        );
    }

    #[test]
    fn long_file_names_are_cut_at_a_character() {
        let name = file_name(&format!("/gcodes/{}.gcode", "ç".repeat(100)));
        assert_eq!(name.len(), 128);
    }
}
