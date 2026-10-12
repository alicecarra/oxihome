use std::time::Instant;

use oxihome_node::mqtt::SoftwareNode;
use oxihome_node::{Device, NodeCore, Rejection};
use oxihome_protocol::{
    Access, Channel, ChannelValue, CommandOp, Confidence, EntityDescriptor, ErrorCode, EventKind,
    Message, Quantity, Unit, Value, ValueType,
};

use crate::docker::{self, HEALTH_OPTIONS};
use crate::sample::{Info, Sample, Sampler};

const CPU_ENTITY: u8 = 1;
const MEMORY_ENTITY: u8 = 2;
const BATTERY_ENTITY: u8 = 3;
const SYSTEM_ENTITY: u8 = 4;
/// Fixed ranges, so the ids stay the same when the machine changes.
pub const FIRST_DISK_ENTITY: u8 = 10;
pub const FIRST_INTERFACE_ENTITY: u8 = 30;
pub const FIRST_CONTAINER_ENTITY: u8 = 50;
const CONTAINER_CHANGE_CHANNEL: u8 = 6;
pub const MAXIMUM_PER_RANGE: usize = 20;

/// A Linux machine as a node: CPU, memory, disks, network interfaces, battery, system and
/// containers.
pub struct MonitorNode {
    core: NodeCore,
    device: MonitorDevice,
    sampler: Sampler,
    started_at: Instant,
}

struct MonitorDevice {
    info: Info,
    disks: usize,
    interfaces: usize,
    containers: usize,
    /// `None` until the first report.
    sample: Option<Sample>,
}

impl MonitorNode {
    pub fn new(
        device_id: String,
        sampler: Sampler,
        disks: usize,
        interfaces: usize,
        containers: usize,
    ) -> Self {
        Self {
            core: NodeCore::new(
                device_id,
                format!("linux-monitor {}", env!("CARGO_PKG_VERSION")),
            ),
            device: MonitorDevice {
                info: sampler.info(),
                disks,
                interfaces,
                containers,
                sample: None,
            },
            sampler,
            started_at: Instant::now(),
        }
    }
}

impl SoftwareNode for MonitorNode {
    fn device_id(&self) -> &str {
        self.core.device_id()
    }

    fn handle(&mut self, message: &Message) -> Vec<Message> {
        self.core.handle(&mut self.device, message)
    }

    /// Reads the machine first on the first connection, so the hub gets values with the `Hello`.
    fn announcement(&mut self) -> Vec<Message> {
        if self.device.sample.is_none() {
            self.device.sample = Some(self.sampler.sample());
        }
        self.core.announcement(&self.device)
    }

    fn report(&mut self) -> Vec<Message> {
        let sample = self.sampler.sample();
        let changes = self.device.container_changes(&sample);
        let battery = sample
            .battery
            .as_ref()
            .map(|battery| battery.charge_percent.round() as u8);
        self.device.sample = Some(sample);
        let uptime_s = self.started_at.elapsed().as_secs() as u32;
        let mut messages = vec![self.core.heartbeat(uptime_s, None, battery)];
        messages.extend(self.core.states(&self.device));
        for (entity, change) in changes {
            messages.push(self.core.event(
                entity,
                CONTAINER_CHANGE_CHANNEL,
                EventKind::Received,
                Some(Value::Text(change.into())),
            ));
        }
        messages
    }
}

impl MonitorDevice {
    /// Containers read both now and in the last report; nothing on the first one.
    fn container_changes(&self, sample: &Sample) -> Vec<(u8, &'static str)> {
        let Some(previous) = &self.sample else {
            return Vec::new();
        };
        previous
            .containers
            .iter()
            .zip(&sample.containers)
            .enumerate()
            .filter_map(|(index, pair)| match pair {
                (Some(before), Some(after)) => Some((index, docker::changes(before, after))),
                _ => None,
            })
            .flat_map(|(index, changes)| {
                let entity = FIRST_CONTAINER_ENTITY + index as u8;
                changes.into_iter().map(move |change| (entity, change))
            })
            .collect()
    }
}

impl Device for MonitorDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        let mut cpu = vec![
            reading(0, 100.0, None, Unit::Percent),
            number(1),
            number(2),
            number(3),
            reading(4, 10_000.0, Some(Quantity::Frequency), Unit::Megahertz),
        ];
        if self.info.has_cpu_temperature {
            cpu.push(temperature(5));
        }
        let mut entities = vec![
            EntityDescriptor {
                id: CPU_ENTITY,
                channels: cpu,
            },
            EntityDescriptor {
                id: MEMORY_ENTITY,
                channels: vec![
                    reading(0, 100.0, None, Unit::Percent),
                    size(1, 4096.0),
                    size(2, 4096.0),
                    size(3, 4096.0),
                    reading(4, 100.0, None, Unit::Percent),
                ],
            },
        ];
        if self.info.has_battery {
            entities.push(EntityDescriptor {
                id: BATTERY_ENTITY,
                channels: vec![reading(0, 100.0, None, Unit::Percent), flag(1), flag(2)],
            });
        }
        entities.push(EntityDescriptor {
            id: SYSTEM_ENTITY,
            channels: vec![
                count(0, Some(Unit::Second)),
                text(1, 64),
                text(2, 64),
                text(3, 128),
            ],
        });
        entities.extend((0..self.disks).map(|index| EntityDescriptor {
            id: FIRST_DISK_ENTITY + index as u8,
            channels: vec![
                reading(0, 100.0, None, Unit::Percent),
                size(1, 1_000_000.0),
                size(2, 1_000_000.0),
            ],
        }));
        entities.extend((0..self.interfaces).map(|index| EntityDescriptor {
            id: FIRST_INTERFACE_ENTITY + index as u8,
            channels: vec![rate(0), rate(1), flag(2), text(3, 45)],
        }));
        let cpu_max = 100.0 * self.info.cpu_count as f32;
        entities.extend((0..self.containers).map(|index| EntityDescriptor {
            id: FIRST_CONTAINER_ENTITY + index as u8,
            channels: vec![
                flag(0),
                Channel {
                    id: 1,
                    access: Access::Read,
                    value: Some(ValueType::Enum(
                        HEALTH_OPTIONS.iter().map(|option| option.to_string()).collect(),
                    )),
                    quantity: None,
                    unit: None,
                },
                reading(2, cpu_max, None, Unit::Percent),
                reading(
                    3,
                    1_000_000.0,
                    Some(Quantity::DataSize),
                    Unit::Megabyte,
                ),
                count(4, None),
                count(5, Some(Unit::Second)),
                Channel {
                    id: CONTAINER_CHANGE_CHANNEL,
                    access: Access::Event,
                    value: Some(ValueType::Text { max_length: 16 }),
                    quantity: None,
                    unit: None,
                },
            ],
        }));
        entities
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        let sample = self.sample.as_ref()?;
        let float = |channel, value: f32| confirmed(channel, Value::Float(value));
        let optional = |channel, value: Option<f32>| value.map(|value| float(channel, value));
        Some(match entity {
            CPU_ENTITY => {
                let cpu = &sample.cpu;
                [
                    optional(0, cpu.usage),
                    Some(float(1, cpu.load[0])),
                    Some(float(2, cpu.load[1])),
                    Some(float(3, cpu.load[2])),
                    Some(float(4, cpu.frequency_mhz)),
                    optional(5, cpu.temperature.filter(|_| self.info.has_cpu_temperature)),
                ]
                .into_iter()
                .flatten()
                .collect()
            }
            MEMORY_ENTITY => {
                let memory = &sample.memory;
                vec![
                    float(0, memory.used_percent),
                    float(1, memory.used_gb),
                    float(2, memory.available_gb),
                    float(3, memory.total_gb),
                    float(4, memory.swap_used_percent),
                ]
            }
            BATTERY_ENTITY if self.info.has_battery => {
                let battery = sample.battery.as_ref()?;
                vec![
                    float(0, battery.charge_percent),
                    confirmed(1, Value::Bool(battery.charging)),
                    confirmed(2, Value::Bool(battery.plugged_in)),
                ]
            }
            SYSTEM_ENTITY => vec![
                confirmed(0, Value::Int(sample.uptime_s.min(i32::MAX as u64) as i32)),
                confirmed(1, Value::Text(self.info.hostname.clone())),
                confirmed(2, Value::Text(self.info.kernel.clone())),
                confirmed(3, Value::Text(self.info.distribution.clone())),
            ],
            id if (FIRST_DISK_ENTITY..FIRST_DISK_ENTITY + self.disks as u8).contains(&id) => {
                let disk = sample.disks[(id - FIRST_DISK_ENTITY) as usize].as_ref()?;
                vec![
                    float(0, disk.used_percent),
                    float(1, disk.free_gb),
                    float(2, disk.total_gb),
                ]
            }
            id if (FIRST_INTERFACE_ENTITY..FIRST_INTERFACE_ENTITY + self.interfaces as u8)
                .contains(&id) =>
            {
                let interface =
                    sample.interfaces[(id - FIRST_INTERFACE_ENTITY) as usize].as_ref()?;
                [
                    optional(0, interface.download_kbps),
                    optional(1, interface.upload_kbps),
                    Some(confirmed(2, Value::Bool(interface.connected))),
                    Some(confirmed(3, Value::Text(interface.address.clone()))),
                ]
                .into_iter()
                .flatten()
                .collect()
            }
            id if (FIRST_CONTAINER_ENTITY..FIRST_CONTAINER_ENTITY + self.containers as u8)
                .contains(&id) =>
            {
                let container =
                    sample.containers[(id - FIRST_CONTAINER_ENTITY) as usize].as_ref()?;
                let int = |channel, value: Option<i64>| {
                    value.map(|value| {
                        confirmed(channel, Value::Int(value.clamp(0, i32::MAX as i64) as i32))
                    })
                };
                [
                    Some(confirmed(0, Value::Bool(container.running))),
                    Some(confirmed(1, Value::Enum(container.health.name().into()))),
                    optional(2, container.cpu_percent),
                    optional(3, container.memory_mb),
                    int(4, container.restarts.map(i64::from)),
                    int(5, container.uptime_s),
                ]
                .into_iter()
                .flatten()
                .collect()
            }
            _ => return None,
        })
    }

    /// Every channel is read only, so the core refuses commands before they get here.
    fn command(&mut self, entity: u8, channel: u8, _op: &CommandOp) -> Result<(), Rejection> {
        Err(Rejection::new(
            ErrorCode::InvalidOperation,
            format!("entity {entity} channel {channel} is read only"),
        ))
    }
}

fn float_channel(
    id: u8,
    max: f32,
    step: f32,
    quantity: Option<Quantity>,
    unit: Option<Unit>,
) -> Channel {
    Channel {
        id,
        access: Access::Read,
        value: Some(ValueType::Float {
            min: 0.0,
            max,
            step,
        }),
        quantity,
        unit,
    }
}

fn reading(id: u8, max: f32, quantity: Option<Quantity>, unit: Unit) -> Channel {
    float_channel(id, max, 0.1, quantity, Some(unit))
}

fn temperature(id: u8) -> Channel {
    Channel {
        value: Some(ValueType::Float {
            min: -50.0,
            max: 150.0,
            step: 0.1,
        }),
        ..float_channel(
            id,
            0.0,
            0.0,
            Some(Quantity::Temperature),
            Some(Unit::Celsius),
        )
    }
}

/// Load average: a plain number.
fn number(id: u8) -> Channel {
    float_channel(id, 1000.0, 0.01, None, None)
}

fn size(id: u8, max: f32) -> Channel {
    float_channel(
        id,
        max,
        0.01,
        Some(Quantity::DataSize),
        Some(Unit::Gigabyte),
    )
}

fn rate(id: u8) -> Channel {
    reading(
        id,
        10_000_000.0,
        Some(Quantity::DataRate),
        Unit::KilobytePerSecond,
    )
}

fn count(id: u8, unit: Option<Unit>) -> Channel {
    Channel {
        id,
        access: Access::Read,
        value: Some(ValueType::Int {
            min: 0,
            max: i32::MAX,
            step: 1,
        }),
        quantity: None,
        unit,
    }
}

fn flag(id: u8) -> Channel {
    Channel {
        id,
        access: Access::Read,
        value: Some(ValueType::Bool),
        quantity: None,
        unit: None,
    }
}

fn text(id: u8, max_length: u16) -> Channel {
    Channel {
        id,
        access: Access::Read,
        value: Some(ValueType::Text { max_length }),
        quantity: None,
        unit: None,
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
    use super::*;
    use crate::docker::{Container, Health};
    use crate::sample::{Battery, Cpu, Disk, Interface, Memory};

    fn device(has_battery: bool) -> MonitorDevice {
        MonitorDevice {
            info: Info {
                hostname: "desk".into(),
                kernel: "6.17.1-arch1-1".into(),
                distribution: "Linux (Arch Linux)".into(),
                has_cpu_temperature: true,
                has_battery,
                cpu_count: 4,
            },
            disks: 2,
            interfaces: 1,
            containers: 2,
            sample: None,
        }
    }

    fn sample(first: bool) -> Sample {
        let rate = |value| (!first).then_some(value);
        Sample {
            cpu: Cpu {
                usage: rate(12.5),
                load: [0.5, 0.4, 0.3],
                frequency_mhz: 2400.0,
                temperature: Some(48.0),
            },
            memory: Memory {
                used_percent: 50.0,
                used_gb: 8.0,
                available_gb: 8.0,
                total_gb: 16.0,
                swap_used_percent: 0.0,
            },
            disks: vec![
                Some(Disk {
                    used_percent: 66.0,
                    free_gb: 170.0,
                    total_gb: 500.0,
                }),
                None,
            ],
            interfaces: vec![Some(Interface {
                download_kbps: rate(120.0),
                upload_kbps: rate(8.0),
                connected: true,
                address: "192.168.0.10".into(),
            })],
            battery: Some(Battery {
                charge_percent: 87.0,
                charging: true,
                plugged_in: true,
            }),
            uptime_s: 3600,
            containers: vec![Some(container(true, Health::Healthy)), None],
        }
    }

    fn container(running: bool, health: Health) -> Container {
        Container {
            running,
            health,
            cpu_percent: Some(if running { 150.0 } else { 0.0 }),
            memory_mb: Some(if running { 80.5 } else { 0.0 }),
            restarts: Some(1),
            uptime_s: Some(if running { 600 } else { 0 }),
        }
    }

    fn ids(device: &MonitorDevice) -> Vec<u8> {
        device.entities().iter().map(|entity| entity.id).collect()
    }

    #[test]
    fn disks_and_interfaces_take_fixed_ranges() {
        assert_eq!(ids(&device(true)), [1, 2, 3, 4, 10, 11, 30, 50, 51]);
        assert_eq!(
            ids(&device(false)),
            [1, 2, 4, 10, 11, 30, 50, 51],
            "no battery entity"
        );
    }

    #[test]
    fn rates_wait_for_the_second_reading() {
        let mut device = device(true);
        device.sample = Some(sample(true));
        let channels = |device: &MonitorDevice, entity| -> Vec<u8> {
            device
                .state(entity)
                .unwrap()
                .iter()
                .map(|value| value.channel)
                .collect()
        };
        assert_eq!(channels(&device, CPU_ENTITY), [1, 2, 3, 4, 5]);
        assert_eq!(channels(&device, FIRST_INTERFACE_ENTITY), [2, 3]);
        device.sample = Some(sample(false));
        assert_eq!(channels(&device, CPU_ENTITY), [0, 1, 2, 3, 4, 5]);
        assert_eq!(channels(&device, FIRST_INTERFACE_ENTITY), [0, 1, 2, 3]);
    }

    #[test]
    fn an_unmounted_disk_has_no_state() {
        let mut device = device(true);
        device.sample = Some(sample(false));
        assert!(device.state(FIRST_DISK_ENTITY).is_some());
        assert!(device.state(FIRST_DISK_ENTITY + 1).is_none());
        assert!(
            device.state(FIRST_DISK_ENTITY + 2).is_none(),
            "not declared"
        );
    }

    #[test]
    fn every_state_fits_the_declaration() {
        let mut device = device(true);
        device.sample = Some(sample(false));
        for entity in device.entities() {
            for value in device.state(entity.id).unwrap_or_default() {
                let channel = entity
                    .channels
                    .iter()
                    .find(|channel| channel.id == value.channel)
                    .unwrap();
                let fits = match (&channel.value, &value.value) {
                    (Some(ValueType::Float { min, max, .. }), Value::Float(v)) => {
                        v >= min && v <= max
                    }
                    (Some(ValueType::Int { min, max, .. }), Value::Int(v)) => v >= min && v <= max,
                    (Some(ValueType::Bool), Value::Bool(_)) => true,
                    (Some(ValueType::Enum(options)), Value::Enum(option)) => {
                        options.contains(option)
                    }
                    (Some(ValueType::Text { max_length }), Value::Text(text)) => {
                        text.len() <= *max_length as usize
                    }
                    _ => false,
                };
                assert!(fits, "entity {} channel {}", entity.id, value.channel);
            }
        }
    }

    #[test]
    fn container_changes_become_events() {
        let mut device = device(true);
        let first = sample(false);
        assert_eq!(
            device.container_changes(&first),
            [],
            "nothing before the first report"
        );
        device.sample = Some(first);

        let mut stopped = sample(false);
        stopped.containers[0] = Some(container(false, Health::None));
        stopped.containers[1] = Some(container(true, Health::Healthy));
        assert_eq!(
            device.container_changes(&stopped),
            [(FIRST_CONTAINER_ENTITY, "stopped")],
            "the second container was not read before"
        );
    }

    #[test]
    fn a_stopped_container_reports_zeros() {
        let mut device = device(true);
        let mut stopped = sample(false);
        stopped.containers[0] = Some(container(false, Health::None));
        device.sample = Some(stopped);
        let channels: Vec<u8> = device
            .state(FIRST_CONTAINER_ENTITY)
            .unwrap()
            .iter()
            .map(|value| value.channel)
            .collect();
        assert_eq!(channels, [0, 1, 2, 3, 4, 5], "zeros, not the last values");
        assert!(
            device.state(FIRST_CONTAINER_ENTITY + 1).is_none(),
            "not read"
        );
    }
}
