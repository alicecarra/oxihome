use std::collections::HashMap;
use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Build, Channel, ChannelValue, Color, ColorModel, CommandOp, Confidence,
    EntityDescriptor, ErrorCode, EventKind, Message, Payload, Quantity, Unit, Value, ValueType,
};
use serde_json::Map;

use crate::creality::{JobState, MaterialBox, Status};

const TEMPERATURES_ENTITY: u8 = 1;
const JOB_ENTITY: u8 = 2;
const PRINTER_ENTITY: u8 = 3;
const CONNECTION_ENTITY: u8 = 4;
/// The Creality Filament System, the first one when there are more.
const BOX_ENTITY: u8 = 5;
const ENTITIES: [u8; 5] = [
    TEMPERATURES_ENTITY,
    JOB_ENTITY,
    PRINTER_ENTITY,
    CONNECTION_ENTITY,
    BOX_ENTITY,
];

const JOB_ELAPSED_CHANNEL: u8 = 4;
const JOB_LEFT_CHANNEL: u8 = 5;
const JOB_CHANGE_CHANNEL: u8 = 7;
const JOB_STARTED_CHANNEL: u8 = 8;
const BOX_TEMPERATURE_CHANNEL: u8 = 0;
const BOX_HUMIDITY_CHANNEL: u8 = 1;
/// 1 to 4 for the slot feeding the printer, 0 for none.
const BOX_ACTIVE_CHANNEL: u8 = 2;
const BOX_SLOTS: u8 = 4;
const MATERIAL_MAX_LENGTH: u16 = 32;
const FILE_MAX_LENGTH: u16 = 128;

/// The printer sends a few updates a second; a channel goes out only when it moved this far from
/// what the hub last got. More than 1 °C hides the nozzle swinging around its target and the
/// chamber flipping between two whole degrees.
const TEMPERATURE_STEP: f32 = 1.0;
const HUMIDITY_STEP: f32 = 2.0;
const TIME_STEP_S: i32 = 300;

pub struct PrinterNode {
    core: NodeCore,
    device: PrinterDevice,
    /// What the hub last got of each channel, by entity and channel.
    published: HashMap<(u8, u8), Value>,
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

        let mut messages = self.changes();
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
        self.changes()
    }

    /// One `State` per entity with only the channels that moved enough.
    fn changes(&mut self) -> Vec<Message> {
        let mut messages = Vec::new();
        for entity in ENTITIES {
            let Some(values) = self.device.state(entity) else {
                continue;
            };
            let values: Vec<ChannelValue> = values
                .into_iter()
                .filter(|new| {
                    self.published
                        .get(&(entity, new.channel))
                        .is_none_or(|old| significant(entity, new.channel, old, &new.value))
                })
                .collect();
            if values.is_empty() {
                continue;
            }
            for value in &values {
                self.published
                    .insert((entity, value.channel), value.value.clone());
            }
            messages.push(self.core.message(Payload::State { entity, values }, None));
        }
        messages
    }
}

fn significant(entity: u8, channel: u8, old: &Value, new: &Value) -> bool {
    match (entity, channel, old, new) {
        (TEMPERATURES_ENTITY, _, Value::Float(old), Value::Float(new))
        | (BOX_ENTITY, BOX_TEMPERATURE_CHANNEL, Value::Float(old), Value::Float(new)) => {
            (old - new).abs() > TEMPERATURE_STEP
        }
        (BOX_ENTITY, BOX_HUMIDITY_CHANNEL, Value::Float(old), Value::Float(new)) => {
            (old - new).abs() > HUMIDITY_STEP
        }
        (JOB_ENTITY, JOB_ELAPSED_CHANNEL | JOB_LEFT_CHANNEL, Value::Int(old), Value::Int(new)) => {
            (old - new).abs() >= TIME_STEP_S
        }
        (_, _, old, new) => old != new,
    }
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
            .flat_map(|entity| {
                self.device
                    .state(entity)
                    .unwrap_or_default()
                    .into_iter()
                    .map(move |value| ((entity, value.channel), value.value))
            })
            .collect();
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        vec![self.core.heartbeat(uptime_s, None, None)]
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
                    read(JOB_STARTED_CHANNEL, integer(i32::MAX), Some(Unit::UnixTime))
                        .named("Started"),
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
            EntityDescriptor {
                id: BOX_ENTITY,
                name: None,
                channels: box_channels(),
            }
            .named("CFS"),
        ]
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        if entity == CONNECTION_ENTITY {
            return Some(vec![confirmed(0, Value::Bool(self.reachable?))]);
        }
        if entity == BOX_ENTITY {
            let values = box_values(&self.status.as_ref()?.material_box()?);
            return (!values.is_empty()).then_some(values);
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
                int(JOB_STARTED_CHANNEL, "printStartTime"),
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

/// Temperature, humidity and the active slot, then color, material and what is left of each slot:
/// slot 1 on channels 10 to 12, slot 2 on 20 to 22...
fn box_channels() -> Vec<Channel> {
    let mut channels = vec![
        reading(
            BOX_TEMPERATURE_CHANNEL,
            0.0,
            100.0,
            Quantity::Temperature,
            Unit::Celsius,
        )
        .named("Temperature"),
        reading(
            BOX_HUMIDITY_CHANNEL,
            0.0,
            100.0,
            Quantity::Humidity,
            Unit::Percent,
        )
        .named("Humidity"),
        read(BOX_ACTIVE_CHANNEL, integer(i32::from(BOX_SLOTS)), None).named("Active slot"),
    ];
    for slot in 1..=BOX_SLOTS {
        let first = slot * 10;
        channels.extend([
            read(first, ValueType::Color(ColorModel::Rgb), None)
                .named(format!("Slot {slot} color")),
            read(
                first + 1,
                ValueType::Text {
                    max_length: MATERIAL_MAX_LENGTH,
                },
                None,
            )
            .named(format!("Slot {slot} material")),
            read(first + 2, integer(100), Some(Unit::Percent)).named(format!("Slot {slot} left")),
        ]);
    }
    channels
}

fn box_values(material_box: &MaterialBox) -> Vec<ChannelValue> {
    let mut values = Vec::new();
    if let Some(temperature) = material_box.temperature {
        values.push(confirmed(
            BOX_TEMPERATURE_CHANNEL,
            Value::Float(temperature as f32),
        ));
    }
    if let Some(humidity) = material_box.humidity {
        values.push(confirmed(
            BOX_HUMIDITY_CHANNEL,
            Value::Float(humidity as f32),
        ));
    }
    let active = material_box
        .slots
        .iter()
        .find(|slot| slot.selected)
        .map_or(0, |slot| i32::from(slot.number));
    values.push(confirmed(BOX_ACTIVE_CHANNEL, Value::Int(active)));
    for slot in material_box
        .slots
        .iter()
        .filter(|slot| (1..=BOX_SLOTS).contains(&slot.number))
    {
        let first = slot.number * 10;
        if let Some((r, g, b)) = slot.color {
            values.push(confirmed(first, Value::Color(Color::Rgb { r, g, b })));
        }
        values.push(confirmed(
            first + 1,
            Value::Text(cut(&slot.material, MATERIAL_MAX_LENGTH).to_string()),
        ));
        values.push(confirmed(first + 2, Value::Int(slot.left)));
    }
    values
}

/// "/usr/data/printer_data/gcodes/Box.stl_PLA_1h2m3s.gcode" → "Box.stl_PLA_1h2m3s".
fn file_name(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.strip_suffix(".gcode").unwrap_or(name);
    cut(name, FILE_MAX_LENGTH).to_string()
}

/// At most `max_length` bytes, ending on a character.
fn cut(text: &str, max_length: u16) -> &str {
    let mut end = text.len().min(usize::from(max_length));
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
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
        assert_eq!(
            states(&messages),
            [
                TEMPERATURES_ENTITY,
                JOB_ENTITY,
                PRINTER_ENTITY,
                CONNECTION_ENTITY
            ],
            "the filament box waits for its answer"
        );
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
    fn only_the_channels_that_moved_enough_go_out() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        let small = node.update(update(json!({
            "nozzleTemp": "221.400000", "boxTemp": 34, "printJobTime": 5400,
        })));
        assert!(small.is_empty());
        assert!(states(&node.report()).is_empty());

        let messages = node.update(update(
            json!({ "nozzleTemp": "221.600000", "layer": 257, "printJobTime": 5430 }),
        ));
        let channels: Vec<(u8, Vec<u8>)> = messages
            .iter()
            .filter_map(|message| match &message.payload {
                Payload::State { entity, values } => {
                    Some((*entity, values.iter().map(|value| value.channel).collect()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            channels,
            [
                (TEMPERATURES_ENTITY, vec![0]),
                (JOB_ENTITY, vec![2, JOB_ELAPSED_CHANNEL])
            ],
            "the drift adds up from the last value sent"
        );
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

    fn boxes() -> Map<String, serde_json::Value> {
        update(json!({ "boxsInfo": {
            "same_material": [],
            "materialBoxs": [
                { "id": 0, "state": 0, "type": 1, "materials": [
                    { "id": 0, "type": "PLA", "color": "#0ff1e1e", "percent": 100, "selected": 0 },
                ] },
                { "id": 1, "state": 1, "type": 0, "temp": 22.0, "humidity": 63.0, "materials": [
                    { "id": 0, "type": "PLA", "color": "#0b2a1e1", "percent": 100, "selected": 0 },
                    { "id": 1, "type": "PETG", "color": "#0ba552a", "percent": 35, "selected": 1 },
                ] },
            ],
        } }))
    }

    #[test]
    fn the_filament_box_comes_from_its_answer() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        let messages = node.update(boxes());
        assert_eq!(states(&messages), [BOX_ENTITY]);
        let values: Vec<(u8, Value)> = node
            .device
            .state(BOX_ENTITY)
            .unwrap()
            .into_iter()
            .map(|value| (value.channel, value.value))
            .collect();
        assert_eq!(
            values,
            [
                (BOX_TEMPERATURE_CHANNEL, Value::Float(22.0)),
                (BOX_HUMIDITY_CHANNEL, Value::Float(63.0)),
                (BOX_ACTIVE_CHANNEL, Value::Int(2)),
                (
                    10,
                    Value::Color(Color::Rgb {
                        r: 0xb2,
                        g: 0xa1,
                        b: 0xe1
                    })
                ),
                (11, Value::Text("PLA".into())),
                (12, Value::Int(100)),
                (
                    20,
                    Value::Color(Color::Rgb {
                        r: 0xba,
                        g: 0x55,
                        b: 0x2a
                    })
                ),
                (21, Value::Text("PETG".into())),
                (22, Value::Int(35)),
            ]
        );
        assert!(
            node.update(boxes()).is_empty(),
            "the same box again sends nothing"
        );
    }

    #[test]
    fn without_a_box_answer_the_entity_stays_empty() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        assert!(node.device.state(BOX_ENTITY).is_none());
    }

    #[test]
    fn the_start_is_a_moment() {
        let mut node = PrinterNode::new("printer".into());
        node.update(printing());
        let messages = node.update(update(json!({ "printStartTime": 1_792_440_000 })));
        assert_eq!(states(&messages), [JOB_ENTITY]);
        let started = node
            .device
            .state(JOB_ENTITY)
            .unwrap()
            .into_iter()
            .find(|value| value.channel == JOB_STARTED_CHANNEL)
            .unwrap();
        assert_eq!(started.value, Value::Int(1_792_440_000));
    }

    #[test]
    fn long_file_names_are_cut_at_a_character() {
        let name = file_name(&format!("/gcodes/{}.gcode", "ç".repeat(100)));
        assert_eq!(name.len(), 128);
    }
}
