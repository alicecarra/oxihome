use oxihome_protocol::{Access, Channel, Confidence, ValueType};

use super::*;

const DEVICE: &str = "test-node";
const LAMP: u8 = 1;
const BUTTON: u8 = 2;
const BUZZER: u8 = 3;
const DIMMER: u8 = 4;

/// A lamp (`Bool`), a button (events), a buzzer (`Trigger` with patterns and a plain one) and a
/// dimmer (`Int` 0..=100 and a `Text` label of up to 5 bytes).
#[derive(Default)]
struct TestDevice {
    lamp: bool,
    level: i32,
    commands: Vec<(u8, u8, CommandOp)>,
    fail_next: bool,
}

fn channel(id: u8, access: Access, value: Option<ValueType>) -> Channel {
    Channel {
        id,
        name: None,
        access,
        value,
        quantity: None,
        unit: None,
    }
}

impl Device for TestDevice {
    fn entities(&self) -> Vec<EntityDescriptor> {
        vec![
            EntityDescriptor {
                id: LAMP,
                name: None,
                channels: vec![channel(0, Access::ReadWrite, Some(ValueType::Bool))],
            },
            EntityDescriptor {
                id: BUTTON,
                name: None,
                channels: vec![channel(0, Access::Event, None)],
            },
            EntityDescriptor {
                id: BUZZER,
                name: None,
                channels: vec![
                    channel(
                        0,
                        Access::Trigger,
                        Some(ValueType::Enum(vec!["Short".into(), "Alarm".into()])),
                    ),
                    channel(1, Access::Trigger, None),
                ],
            },
            EntityDescriptor {
                id: DIMMER,
                name: None,
                channels: vec![
                    channel(
                        0,
                        Access::ReadWrite,
                        Some(ValueType::Int {
                            min: 0,
                            max: 100,
                            step: 1,
                        }),
                    ),
                    channel(1, Access::Trigger, Some(ValueType::Text { max_length: 5 })),
                ],
            },
        ]
    }

    fn state(&self, entity: u8) -> Option<Vec<ChannelValue>> {
        let value = match entity {
            LAMP => Value::Bool(self.lamp),
            DIMMER => Value::Int(self.level),
            _ => return None,
        };
        Some(vec![ChannelValue {
            channel: 0,
            value,
            confidence: Confidence::Confirmed,
        }])
    }

    fn command(&mut self, entity: u8, channel: u8, op: &CommandOp) -> Result<(), Rejection> {
        if std::mem::take(&mut self.fail_next) {
            return Err(Rejection::new(ErrorCode::Internal, "GPIO error"));
        }
        self.commands.push((entity, channel, op.clone()));
        match (entity, op) {
            (LAMP, CommandOp::Set(Value::Bool(on))) => self.lamp = *on,
            (DIMMER, CommandOp::Set(Value::Int(level))) => self.level = *level,
            _ => {}
        }
        Ok(())
    }
}

fn core() -> NodeCore {
    NodeCore::new(DEVICE, Build::new("test", "1.0.0", "test"))
}

fn from_hub(message_id: u32, payload: Payload) -> Message {
    Message::new("oxihome-hub", message_id, payload).to(DEVICE)
}

fn command(entity: u8, channel: u8, op: CommandOp) -> Message {
    from_hub(
        7,
        Payload::Command {
            entity,
            channel,
            op,
        },
    )
}

/// The error code of a single `Error` answer to message 7.
fn error_code(answers: &[Message]) -> Option<ErrorCode> {
    match answers {
        [
            Message {
                reply_to: Some(7),
                payload: Payload::Error { code, .. },
                ..
            },
        ] => [
            ErrorCode::InvalidMessage,
            ErrorCode::Unsupported,
            ErrorCode::UnknownEntity,
            ErrorCode::UnknownChannel,
            ErrorCode::InvalidValue,
            ErrorCode::InvalidOperation,
            ErrorCode::Internal,
        ]
        .into_iter()
        .find(|known| u16::from(*known) == *code),
        _ => None,
    }
}

fn refusal(op_entity: u8, op_channel: u8, op: CommandOp) -> Option<ErrorCode> {
    let mut device = TestDevice::default();
    let answers = core().handle(&mut device, &command(op_entity, op_channel, op));
    assert!(device.commands.is_empty(), "refused commands must not run");
    error_code(&answers)
}

#[test]
fn set_answers_ack_then_state() {
    let mut device = TestDevice::default();
    let answers = core().handle(
        &mut device,
        &command(LAMP, 0, CommandOp::Set(Value::Bool(true))),
    );

    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0].payload, Payload::Ack { ok: true });
    assert_eq!(answers[0].reply_to, Some(7));
    assert_eq!(
        answers[1].payload,
        Payload::State {
            entity: LAMP,
            values: vec![ChannelValue {
                channel: 0,
                value: Value::Bool(true),
                confidence: Confidence::Confirmed,
            }],
        }
    );
    assert!(device.lamp);
}

#[test]
fn invoke_answers_only_ack() {
    let mut device = TestDevice::default();
    let alarm = CommandOp::Invoke(Some(Value::Enum("Alarm".into())));
    let answers = core().handle(&mut device, &command(BUZZER, 0, alarm.clone()));

    assert_eq!(answers.len(), 1);
    assert_eq!(answers[0].payload, Payload::Ack { ok: true });
    assert_eq!(device.commands, vec![(BUZZER, 0, alarm)]);
}

#[test]
fn unknown_entity_and_channel() {
    let on = || CommandOp::Set(Value::Bool(true));
    assert_eq!(refusal(9, 0, on()), Some(ErrorCode::UnknownEntity));
    assert_eq!(refusal(LAMP, 5, on()), Some(ErrorCode::UnknownChannel));
}

#[test]
fn access_decides_which_operations_are_allowed() {
    assert_eq!(
        refusal(BUTTON, 0, CommandOp::Set(Value::Bool(true))),
        Some(ErrorCode::InvalidOperation)
    );
    assert_eq!(
        refusal(LAMP, 0, CommandOp::Invoke(None)),
        Some(ErrorCode::InvalidOperation)
    );
    assert_eq!(
        refusal(BUZZER, 1, CommandOp::Set(Value::Bool(true))),
        Some(ErrorCode::InvalidOperation)
    );
}

#[test]
fn values_must_fit_the_declared_type() {
    let invalid = Some(ErrorCode::InvalidValue);
    assert_eq!(refusal(LAMP, 0, CommandOp::Set(Value::Int(1))), invalid);
    assert_eq!(refusal(DIMMER, 0, CommandOp::Set(Value::Int(101))), invalid);
    assert_eq!(
        refusal(
            BUZZER,
            0,
            CommandOp::Invoke(Some(Value::Enum("Siren".into())))
        ),
        invalid
    );
    assert_eq!(refusal(BUZZER, 0, CommandOp::Invoke(None)), invalid);
    assert_eq!(
        refusal(BUZZER, 1, CommandOp::Invoke(Some(Value::Bool(true)))),
        invalid
    );
    assert_eq!(
        refusal(
            DIMMER,
            1,
            CommandOp::Invoke(Some(Value::Text("hello!".into())))
        ),
        invalid
    );
}

#[test]
fn values_inside_the_declaration_run() {
    let mut device = TestDevice::default();
    let mut core = core();
    for (message_id, (entity, channel, op)) in (10..).zip([
        (DIMMER, 0, CommandOp::Set(Value::Int(100))),
        (
            DIMMER,
            1,
            CommandOp::Invoke(Some(Value::Text("hello".into()))),
        ),
        (BUZZER, 1, CommandOp::Invoke(None)),
    ]) {
        let message = from_hub(
            message_id,
            Payload::Command {
                entity,
                channel,
                op,
            },
        );
        let answers = core.handle(&mut device, &message);
        assert_eq!(answers[0].payload, Payload::Ack { ok: true });
    }
    assert_eq!(device.commands.len(), 3);
}

#[test]
fn device_failures_are_reported() {
    let mut device = TestDevice {
        fail_next: true,
        ..TestDevice::default()
    };
    let answers = core().handle(
        &mut device,
        &command(LAMP, 0, CommandOp::Set(Value::Bool(true))),
    );
    assert_eq!(error_code(&answers), Some(ErrorCode::Internal));
}

#[test]
fn duplicate_commands_get_the_same_ack_without_running_again() {
    let mut device = TestDevice::default();
    let mut core = core();
    let message = command(LAMP, 0, CommandOp::Set(Value::Bool(true)));
    let first = core.handle(&mut device, &message);
    let again = core.handle(&mut device, &message);

    assert_eq!(again, vec![first[0].clone()]);
    assert_eq!(device.commands.len(), 1);
}

#[test]
fn messages_for_other_nodes_are_ignored() {
    let mut message = command(LAMP, 0, CommandOp::Set(Value::Bool(true)));
    message.dst = Some("someone-else".into());
    assert!(
        core()
            .handle(&mut TestDevice::default(), &message)
            .is_empty()
    );
}

#[test]
fn broadcast_queries_are_answered() {
    let query = Message::new(
        "oxihome-hub",
        3,
        Payload::Query {
            what: QueryKind::Hello,
        },
    );
    let answers = core().handle(&mut TestDevice::default(), &query);
    assert!(matches!(
        &answers[..],
        [Message {
            reply_to: Some(3),
            payload: Payload::Hello { build, entities },
            ..
        }] if build.version == "1.0.0" && entities.len() == 4
    ));
}

#[test]
fn state_queries() {
    let mut device = TestDevice::default();
    let mut core = core();
    let query = |entity| {
        from_hub(
            7,
            Payload::Query {
                what: QueryKind::State { entity },
            },
        )
    };
    let entities: Vec<u8> = core
        .handle(&mut device, &query(None))
        .iter()
        .filter_map(|message| match message.payload {
            Payload::State { entity, .. } => Some(entity),
            _ => None,
        })
        .collect();
    assert_eq!(entities, vec![LAMP, DIMMER]);
    assert_eq!(
        error_code(&core.handle(&mut device, &query(Some(BUTTON)))),
        Some(ErrorCode::InvalidOperation)
    );
    assert_eq!(
        error_code(&core.handle(&mut device, &query(Some(9)))),
        Some(ErrorCode::UnknownEntity)
    );
}

#[test]
fn messages_are_numbered_until_reset() {
    let mut core = core();
    let device = TestDevice::default();
    let ids: Vec<u32> = core
        .announcement(&device)
        .iter()
        .map(|message| message.msg_id)
        .collect();
    assert_eq!(ids, vec![0, 1, 2]);
    assert_eq!(core.heartbeat(10, Some(-50), None).msg_id, 3);
    core.reset();
    assert_eq!(core.event(BUTTON, 0, EventKind::Press, None).msg_id, 0);
}

#[test]
fn unsupported_messages_are_refused() {
    let answers = core().handle(
        &mut TestDevice::default(),
        &from_hub(7, Payload::Ack { ok: true }),
    );
    assert_eq!(error_code(&answers), Some(ErrorCode::Unsupported));
}
