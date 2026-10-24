//! The draft written as the TOML that goes in `rules.toml`.

use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table, value};

use super::Built;
use super::catalog::{Catalog, Control};
use super::draft::{
    ActionDraft, ActionKind, ChannelKey, ConditionDraft, ConditionKind, DAYS, Draft, Kind,
    TriggerDraft, TriggerKind, parse_pair,
};

fn required<'a>(text: &'a str, missing: &str) -> Built<&'a str> {
    let text = text.trim();
    if text.is_empty() {
        Err(missing.into())
    } else {
        Ok(text)
    }
}

pub(super) fn number(text: &str, what: &str) -> Built<f64> {
    let text = required(text, &format!("write {what}"))?;
    text.replace(',', ".")
        .parse()
        .map_err(|_| format!("{what} must be a number, not \"{text}\""))
}

fn insert_channel(table: &mut InlineTable, key: &str) -> Built<()> {
    let key = ChannelKey::parse(key).ok_or("choose the channel")?;
    table.insert("device", key.device.as_str().into());
    table.insert("entity", i64::from(key.entity).into());
    table.insert("channel", i64::from(key.channel).into());
    Ok(())
}

fn day_array(days: &[String]) -> Array {
    DAYS.iter()
        .filter(|(day, _)| days.iter().any(|chosen| chosen == day))
        .map(|(day, _)| *day)
        .collect()
}

/// One item per line, like the rules written by hand.
pub(super) fn list(items: Vec<InlineTable>) -> Array {
    let mut array: Array = items.into_iter().collect();
    if !array.is_empty() {
        for item in array.iter_mut() {
            item.decor_mut().set_prefix("\n    ");
        }
        array.set_trailing_comma(true);
        array.set_trailing("\n");
    }
    array
}

fn trigger_table(trigger: &TriggerDraft, catalog: &Catalog) -> Built<InlineTable> {
    let mut table = InlineTable::new();
    match trigger.kind {
        TriggerKind::Event => {
            insert_channel(&mut table, &trigger.channel)?;
            table.insert("event", trigger.event.as_str().into());
            let data = trigger.data.trim();
            if !data.is_empty() {
                table.insert("data", data.into());
            }
        }
        TriggerKind::Threshold => {
            insert_channel(&mut table, &trigger.channel)?;
            if trigger.direction == "equals" {
                let control = Control::for_condition(catalog.declared(&trigger.channel));
                table.insert("equals", expected(&control, &trigger.limit)?);
            } else {
                let limit = number(&trigger.limit, "the limit")?;
                table.insert(trigger.direction.as_str(), limit.into());
            }
            insert_lasting(&mut table, &trigger.lasting);
        }
        TriggerKind::Schedule => {
            table.insert("at", required(&trigger.at, "choose the time")?.into());
            if !trigger.days.is_empty() {
                table.insert("days", day_array(&trigger.days).into());
            }
        }
        TriggerKind::Cron => {
            let expression = required(&trigger.cron, "write the cron expression")?;
            table.insert("cron", expression.into());
        }
        TriggerKind::Offline => {
            table.insert(
                "offline",
                required(&trigger.device, "choose the node")?.into(),
            );
            insert_lasting(&mut table, &trigger.lasting);
        }
        TriggerKind::Notice => {
            table.insert(
                "notice",
                required(&trigger.device, "choose the node")?.into(),
            );
        }
        TriggerKind::Traffic => {
            table.insert(
                trigger.alert.as_str(),
                required(&trigger.device, "choose the node")?.into(),
            );
        }
        TriggerKind::RemoteKey => {
            let (remote, key) = parse_pair(&trigger.remote_key).ok_or("choose the remote key")?;
            table.insert("remote", remote.into());
            table.insert("key", key.into());
        }
        TriggerKind::Button => {
            table.insert(
                "button",
                required(&trigger.button, "write the button's name")?.into(),
            );
        }
    }
    Ok(table)
}

fn insert_lasting(table: &mut InlineTable, lasting: &str) {
    let lasting = lasting.trim();
    if !lasting.is_empty() {
        table.insert("for", lasting.into());
    }
}

fn condition_table(condition: &ConditionDraft, catalog: &Catalog) -> Built<InlineTable> {
    let mut table = InlineTable::new();
    match condition.kind {
        ConditionKind::Channel => {
            insert_channel(&mut table, &condition.channel)?;
            if condition.test == "equals" {
                let control = Control::for_condition(catalog.declared(&condition.channel));
                table.insert("equals", expected(&control, &condition.value)?);
            } else {
                let limit = number(&condition.value, "the limit")?;
                table.insert(condition.test.as_str(), limit.into());
            }
        }
        ConditionKind::Time => {
            table.insert(
                "from",
                required(&condition.from, "choose the start time")?.into(),
            );
            table.insert("to", required(&condition.to, "choose the end time")?.into());
        }
        ConditionKind::Days => {
            if condition.days.is_empty() {
                return Err("choose at least one day".into());
            }
            table.insert("days", day_array(&condition.days).into());
        }
        ConditionKind::Online | ConditionKind::Offline => {
            let device = required(&condition.device, "choose the node")?;
            table.insert(condition.kind.key(), device.into());
        }
    }
    Ok(table)
}

/// A channel the hub does not know takes `true`/`false`, a number or text, as written.
pub(super) fn expected(control: &Control, text: &str) -> Built<toml_edit::Value> {
    let text = text.trim();
    match control {
        Control::OnOff => Ok((text == "true").into()),
        Control::Number => number(text, "the value").map(Into::into),
        Control::Options(_) | Control::Nothing => Ok(required(text, "choose the value")?.into()),
        Control::Text => {
            let text = required(text, "write the value")?;
            Ok(match text {
                "true" => true.into(),
                "false" => false.into(),
                _ => match text.parse::<f64>() {
                    Ok(number) => number.into(),
                    Err(_) => text.into(),
                },
            })
        }
    }
}

fn action_table(action: &ActionDraft) -> Built<InlineTable> {
    let mut table = InlineTable::new();
    if action.kind == ActionKind::Wait {
        let wait = required(&action.wait, "write how long, e.g. 2s")?;
        table.insert("wait", wait.into());
        return Ok(table);
    }
    let mut inner = InlineTable::new();
    match action.kind {
        ActionKind::Set => {
            insert_channel(&mut inner, &action.channel)?;
            inner.insert("value", (action.value != "false").into());
        }
        ActionKind::Toggle => insert_channel(&mut inner, &action.channel)?,
        ActionKind::Invoke => {
            insert_channel(&mut inner, &action.channel)?;
            let argument = action.value.trim();
            if !argument.is_empty() {
                inner.insert("value", argument.into());
            }
        }
        ActionKind::Notify => {
            inner.insert("title", required(&action.title, "write the title")?.into());
            inner.insert("message", action.message.trim().into());
        }
        ActionKind::Wait => unreachable!("handled above"),
    }
    table.insert(action.kind.key(), inner.into());
    Ok(table)
}

/// Prefixed by its number, for errors: `Condition 2: choose the node`.
fn numbered<T>(word: &str, items: impl Iterator<Item = Built<T>>) -> Built<Vec<T>> {
    items
        .enumerate()
        .map(|(index, item)| item.map_err(|error| format!("{word} {}: {error}", index + 1)))
        .collect()
}

pub(super) fn rule_table(draft: &Draft, catalog: &Catalog) -> Built<Table> {
    let mut table = Table::new();
    table.insert(
        "name",
        value(required(&draft.name, "write the rule's name")?),
    );
    if !draft.enabled {
        table.insert("enabled", value(false));
    }
    let when = trigger_table(&draft.trigger, catalog).map_err(|error| format!("When: {error}"))?;
    table.insert("when", value(when));
    let cooldown = draft.cooldown.trim();
    if !cooldown.is_empty() && draft.trigger.kind != TriggerKind::Button {
        table.insert("cooldown", value(cooldown));
    }
    let conditions = numbered(
        "Condition",
        draft
            .conditions
            .iter()
            .map(|condition| condition_table(condition, catalog)),
    )?;
    if !conditions.is_empty() {
        table.insert("if", value(list(conditions)));
    }
    let actions = numbered("Action", draft.actions.iter().map(action_table))?;
    table.insert("then", value(list(actions)));
    Ok(table)
}

pub(super) fn rule_text(table: &Table) -> String {
    let mut rules = ArrayOfTables::new();
    rules.push(table.clone());
    let mut document = DocumentMut::new();
    document.insert("rules", Item::ArrayOfTables(rules));
    document.to_string()
}
