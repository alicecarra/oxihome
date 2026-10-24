use jiff::civil::time;
use oxihome_protocol::Payload;

use super::*;

fn zone() -> TimeZone {
    TimeZone::get("America/Sao_Paulo").unwrap()
}

fn at(text: &str) -> Timestamp {
    text.parse::<jiff::civil::DateTime>()
        .unwrap()
        .to_zoned(zone())
        .unwrap()
        .timestamp()
}

fn medicine() -> Reminder {
    Reminder {
        entity: 2,
        name: "Morning medicine".into(),
        schedule: Schedule::cron("30 8 * * *").unwrap(),
        repeat: Some(SignedDuration::from_mins(15)),
    }
}

fn plants() -> Reminder {
    Reminder {
        entity: 3,
        name: "Water the plants".into(),
        schedule: Schedule::every("3d", Some(time(9, 0, 0, 0))).unwrap(),
        repeat: None,
    }
}

fn node(reminders: Vec<Reminder>, quiet: Option<Quiet>, now: &str) -> RemindersNode {
    RemindersNode::new(
        "reminders".into(),
        reminders,
        zone(),
        quiet,
        SavedState::default(),
        None,
        at(now),
    )
}

fn invoke(message_id: u32, entity: u8, channel: u8, value: Option<Value>) -> Message {
    Message::new(
        "oxihome-hub",
        message_id,
        Payload::Command {
            entity,
            channel,
            op: CommandOp::Invoke(value),
        },
    )
    .to("reminders")
}

/// The notices, as `title: message`.
fn texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| match &message.payload {
            Payload::Notice { title, message, .. } => Some(format!("{title}: {message}")),
            _ => None,
        })
        .collect()
}

fn pending(node: &RemindersNode, entity: u8) -> bool {
    matches!(
        node.device.state(entity).unwrap()[0].value,
        Value::Bool(true)
    )
}

#[test]
fn falls_due_once_with_its_name_and_a_due_event() {
    let mut node = node(vec![medicine()], None, "2026-10-23T07:00");
    assert!(node.tick(at("2026-10-23T08:29")).is_empty());

    let messages = node.tick(at("2026-10-23T08:30"));
    assert_eq!(texts(&messages), ["Morning medicine: Due at 08:30"]);
    assert!(messages.iter().any(|message| matches!(
        &message.payload,
        Payload::Notice { level: NoticeLevel::Warning, key: Some(key), .. } if key == "2"
    )));
    assert!(messages.iter().any(|message| matches!(
        message.payload,
        Payload::Event {
            entity: 2,
            channel: DUE_CHANNEL,
            event: EventKind::Press,
            ..
        }
    )));
    assert!(pending(&node, 2));
    assert_eq!(
        node.device.state(ALL_REMINDERS_ENTITY).unwrap()[0].value,
        Value::Int(1)
    );
    assert!(node.tick(at("2026-10-23T08:31")).is_empty());
}

#[test]
fn repeats_while_pending_saying_since_when() {
    let mut node = node(vec![medicine()], None, "2026-10-23T07:00");
    node.tick(at("2026-10-23T08:30"));
    assert!(node.tick(at("2026-10-23T08:44")).is_empty());
    assert_eq!(
        texts(&node.tick(at("2026-10-23T08:45"))),
        ["Morning medicine: Pending since 08:30"]
    );
    assert_eq!(
        texts(&node.tick(at("2026-10-24T07:00"))),
        ["Morning medicine: Pending since 23/10 08:30"]
    );
}

#[test]
fn without_repeat_it_goes_out_once() {
    let mut node = node(vec![plants()], None, "2026-10-23T20:00");
    assert_eq!(
        texts(&node.tick(at("2026-10-26T09:00"))),
        ["Water the plants: Due at 09:00"]
    );
    assert!(texts(&node.tick(at("2026-10-27T09:00"))).is_empty());
    assert!(pending(&node, 3));
}

#[test]
fn quiet_hours_hold_it_until_they_end() {
    let quiet = Quiet {
        from: time(22, 0, 0, 0),
        to: time(8, 0, 0, 0),
    };
    let reminder = Reminder {
        schedule: Schedule::cron("0 6 * * *").unwrap(),
        ..medicine()
    };
    let mut node = node(vec![reminder], Some(quiet), "2026-10-23T05:00");
    let messages = node.tick(at("2026-10-23T06:00"));
    assert!(texts(&messages).is_empty());
    assert!(pending(&node, 2), "pending shows even when quiet");
    assert!(texts(&node.tick(at("2026-10-23T07:59"))).is_empty());
    assert_eq!(
        texts(&node.tick(at("2026-10-23T08:00"))),
        ["Morning medicine: Pending since 06:00"]
    );
}

#[test]
fn done_clears_it_and_counts_from_then() {
    let mut node = node(vec![plants()], None, "2026-10-23T20:00");
    node.tick(at("2026-10-26T09:00"));
    let answers = node.handle_at(&invoke(1, 3, DONE_CHANNEL, None), at("2026-10-26T18:00"));
    assert!(matches!(answers[0].payload, Payload::Ack { ok: true }));
    assert!(!pending(&node, 3));
    let states: Vec<u8> = answers
        .iter()
        .filter_map(|message| match message.payload {
            Payload::State { entity, .. } => Some(entity),
            _ => None,
        })
        .collect();
    assert_eq!(states, [3, ALL_REMINDERS_ENTITY]);
    let values = node.device.state(3).unwrap();
    assert!(values.contains(&confirmed(
        LAST_DONE_CHANNEL,
        unix_time(at("2026-10-26T18:00"))
    )));
    assert!(values.contains(&confirmed(
        NEXT_DUE_CHANNEL,
        unix_time(at("2026-10-29T09:00"))
    )));
}

#[test]
fn done_shortly_before_a_fixed_time_counts_for_it() {
    let mut node = node(vec![medicine()], None, "2026-10-23T06:00");
    node.handle_at(&invoke(1, 2, DONE_CHANNEL, None), at("2026-10-23T08:10"));
    assert!(texts(&node.tick(at("2026-10-23T08:30"))).is_empty());
    assert!(!pending(&node, 2));
    assert_eq!(
        texts(&node.tick(at("2026-10-24T08:30"))),
        ["Morning medicine: Due at 08:30"]
    );
}

#[test]
fn done_long_before_a_fixed_time_does_not() {
    let mut node = node(vec![medicine()], None, "2026-10-23T05:00");
    node.handle_at(&invoke(1, 2, DONE_CHANNEL, None), at("2026-10-23T06:00"));
    assert_eq!(
        texts(&node.tick(at("2026-10-23T08:30"))),
        ["Morning medicine: Due at 08:30"]
    );
}

#[test]
fn several_missed_times_make_one_pending() {
    let mut node = node(vec![medicine()], None, "2026-10-20T07:00");
    let messages = node.tick(at("2026-10-23T09:00"));
    assert_eq!(
        texts(&messages),
        ["Morning medicine: Pending since 20/10 08:30"]
    );
    let values = node.device.state(2).unwrap();
    assert!(values.contains(&confirmed(
        NEXT_DUE_CHANNEL,
        unix_time(at("2026-10-20T08:30"))
    )));
}

#[test]
fn a_reaction_naming_one_reminder_marks_it_done() {
    let mut node = node(vec![medicine(), plants()], None, "2026-10-23T07:00");
    node.tick(at("2026-10-26T09:00"));
    assert!(pending(&node, 2) && pending(&node, 3));

    let text = |text: &str| Some(Value::Text(text.into()));
    let answers = node.handle_at(
        &invoke(
            1,
            ALL_REMINDERS_ENTITY,
            REACTION_CHANNEL,
            text("⏰ Weather is nice"),
        ),
        at("2026-10-26T09:05"),
    );
    assert!(matches!(answers[0].payload, Payload::Ack { ok: true }));
    assert_eq!(answers.len(), 1, "nothing changed");

    node.handle_at(
        &invoke(
            2,
            ALL_REMINDERS_ENTITY,
            REACTION_CHANNEL,
            text("Pending: Morning medicine, Water the plants"),
        ),
        at("2026-10-26T09:05"),
    );
    assert!(
        pending(&node, 2) && pending(&node, 3),
        "two names, none done"
    );

    node.handle_at(
        &invoke(
            3,
            ALL_REMINDERS_ENTITY,
            REACTION_CHANNEL,
            text("👍 ⏰ Morning medicine: Pending since 08:30"),
        ),
        at("2026-10-26T09:05"),
    );
    assert!(!pending(&node, 2) && pending(&node, 3));
}

#[test]
fn lists_what_is_pending() {
    let mut node = node(vec![medicine(), plants()], None, "2026-10-23T07:00");
    let list = |node: &mut RemindersNode, id| {
        texts(&node.handle_at(
            &invoke(id, ALL_REMINDERS_ENTITY, LIST_PENDING_CHANNEL, None),
            at("2026-10-23T09:00"),
        ))
    };
    assert_eq!(list(&mut node, 1), ["Pending reminders: none"]);
    node.tick(at("2026-10-23T08:30"));
    assert_eq!(list(&mut node, 2), ["Pending reminders: Morning medicine"]);
}

#[test]
fn done_is_refused_on_all_reminders() {
    let mut node = node(vec![medicine()], None, "2026-10-23T07:00");
    let answers = node.handle_at(
        &invoke(1, ALL_REMINDERS_ENTITY, DONE_CHANNEL, None),
        at("2026-10-23T07:01"),
    );
    assert!(matches!(answers[0].payload, Payload::Error { .. }));
}

#[test]
fn the_state_survives_a_restart() {
    let directory = std::env::temp_dir().join(format!("oxihome-reminders-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("reminders-state.json");
    let start = |saved| {
        RemindersNode::new(
            "reminders".into(),
            vec![medicine()],
            zone(),
            None,
            saved,
            Some(path.clone()),
            at("2026-10-23T07:00"),
        )
    };
    let mut first = start(read_state(&path));
    first.tick(at("2026-10-23T08:30"));

    let mut second = start(read_state(&path));
    assert!(pending(&second, 2));
    assert!(
        texts(&second.tick(at("2026-10-23T08:40"))).is_empty(),
        "already sent"
    );
    assert_eq!(texts(&second.tick(at("2026-10-23T08:45"))).len(), 1);

    std::fs::write(&path, "not json").unwrap();
    assert!(read_state(&path).reminders.is_empty());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn snoozing_quiets_it_until_then() {
    let mut node = node(vec![medicine()], None, "2026-10-23T07:00");
    node.tick(at("2026-10-23T08:30"));
    let snooze = |node: &mut RemindersNode, id, option: &str, now| {
        node.handle_at(
            &invoke(id, 2, SNOOZE_CHANNEL, Some(Value::Enum(option.into()))),
            at(now),
        )
    };
    let answers = snooze(&mut node, 1, "1h", "2026-10-23T08:35");
    assert!(matches!(answers[0].payload, Payload::Ack { ok: true }));
    assert!(pending(&node, 2), "snoozed is still pending");
    assert!(node.device.state(2).unwrap().contains(&confirmed(
        NEXT_DUE_CHANNEL,
        unix_time(at("2026-10-23T09:35"))
    )));
    assert!(texts(&node.tick(at("2026-10-23T09:30"))).is_empty());
    assert_eq!(
        texts(&node.tick(at("2026-10-23T09:35"))),
        ["Morning medicine: Pending since 08:30"]
    );
    assert!(texts(&node.tick(at("2026-10-23T09:40"))).is_empty());
    assert_eq!(
        texts(&node.tick(at("2026-10-23T09:50"))).len(),
        1,
        "repeats again"
    );

    snooze(&mut node, 2, "tomorrow", "2026-10-23T10:00");
    assert!(node.device.state(2).unwrap().contains(&confirmed(
        NEXT_DUE_CHANNEL,
        unix_time(at("2026-10-24T10:00"))
    )));
    node.handle_at(&invoke(3, 2, DONE_CHANNEL, None), at("2026-10-23T10:05"));
    assert!(!pending(&node, 2));
    assert!(
        node.device.state(2).unwrap().contains(&confirmed(
            NEXT_DUE_CHANNEL,
            unix_time(at("2026-10-24T08:30"))
        )),
        "done ends the snooze"
    );
}

#[test]
fn only_a_pending_reminder_snoozes() {
    let mut node = node(vec![medicine()], None, "2026-10-23T07:00");
    let answers = node.handle_at(
        &invoke(1, 2, SNOOZE_CHANNEL, Some(Value::Enum("1h".into()))),
        at("2026-10-23T07:30"),
    );
    assert!(matches!(answers[0].payload, Payload::Error { .. }));
}

#[test]
fn a_sleeping_reaction_snoozes_for_an_hour() {
    let mut node = node(vec![medicine()], None, "2026-10-23T07:00");
    node.tick(at("2026-10-23T08:30"));
    node.handle_at(
        &invoke(
            1,
            ALL_REMINDERS_ENTITY,
            REACTION_CHANNEL,
            Some(Value::Text("😴 ⏰ Morning medicine: Due at 08:30".into())),
        ),
        at("2026-10-23T08:31"),
    );
    assert!(pending(&node, 2));
    assert!(node.device.state(2).unwrap().contains(&confirmed(
        NEXT_DUE_CHANNEL,
        unix_time(at("2026-10-23T09:31"))
    )));
}

#[test]
fn state_saved_by_0_1_0_still_reads() {
    let old = r#"{"reminders":{"2":{"since":"2026-10-23T10:00:00Z","last_done":null,"pending_since":null,"last_sent":null}}}"#;
    let saved: SavedState = serde_json::from_str(old).unwrap();
    assert_eq!(saved.reminders[&2].snoozed_until, None);
}
