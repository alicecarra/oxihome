# Reminders node

`reminders/` (`oxihome-reminders`) is a software node with reminders that fall due at fixed times
(`cron`) or some time after they were last done (`every`, e.g. water the plants 3 days later). A
pending reminder goes out as a `Notice` (see [Node library](library.md)) and again every `repeat` until it is
done, never inside the quiet hours (what falls there goes out when they end). Done comes from the
hub (a button, the dashboard's `reminders` block, a rule) or from a reaction to its Telegram
message; a pending reminder can be snoozed (15 min, 1 h, 3 h or tomorrow at the same time), and 😴
as the reaction snoozes it for an hour. The plan and its decisions are in the Trilium note "Nó de
lembretes".

```sh
cp reminders/reminders.toml.example reminders/reminders.toml   # then the broker password and the reminders
chmod 600 reminders/reminders.toml
infra/mosquitto/set-password.sh reminders   # the password goes in [mqtt] password
cargo run -p oxihome-reminders -- reminders/reminders.toml
```

`reminders/reminders.toml` stays out of git because it holds the broker password; the node keeps
`reminders-state.json` next to it (last done, pending, last sent), so a restart neither forgets a
pending reminder nor sends it again early. A new reminder starts counting at the node's start,
without a warning. When the node was stopped while a reminder fell due, it sends one warning, not
one per missed time. Done before a fixed time counts for it when that time is less than 2 hours
away (the 8:30 medicine taken at 8:10 is not sent at 8:30).

A snoozed reminder stays pending, quiet until the snooze ends and then sent again; done ends the
snooze, and only a pending reminder can be snoozed.

Each warning is a `Notice` of level `Warning`, with the reminder's name as `title`, "Due at 08:30"
(or "Pending since 08:30" when repeated) as `message`, and the reminder's entity as `key`, so the
hub keeps one toast per reminder. Entities: 1 all reminders: 0 how many are pending (`Int`), 2 list
pending (`Trigger`: sends an `Info` notice "Pending reminders" with the names, or "none"), 3
reaction (`Trigger` with `Text`, the Telegram reaction event's text: when it names exactly one
reminder, 😴 at the start snoozes it for an hour and anything else marks it done; a text with no
reminder's name or with more than one is ignored, so no reminder name may contain another). Each
reminder, from entity 2: 0 pending (`Bool`), 1 done (`Trigger`), 2 last done and 3 next due
(`UnixTime`; while snoozed, when the snooze ends), 4 due (`Event`, `Press` when it falls due), 5
snooze (`Trigger` with `Enum` `15m`, `1h`, `3h`, `tomorrow`). Until 0.2.0 the warning was an event
on entity 1 channel 1 and channel 3 was called "done by text". The rules that link it to Telegram,
one for all reminders each:

```toml
[[rules]]
name = "Reminders: Telegram"
when = { notice = "reminders" }
then = [{ invoke = { device = "telegram", entity = 1, channel = 0, value = "⏰ {title}: {message}" } }]

[[rules]]
name = "Reminders: reaction on Telegram"
when = { device = "telegram", entity = 4, channel = 0, event = "Received" }
then = [{ invoke = { device = "reminders", entity = 1, channel = 3, value = "{value}" } }]

[[rules]]
name = "Reminders: list from Telegram"
when = { device = "telegram", entity = 2, channel = 0, event = "Received", data = "lembretes" }
then = [{ invoke = { device = "reminders", entity = 1, channel = 2 } }]
```

The dashboard's `reminders` block (`device = "reminders"`) lists them with Done and Snooze; it needs
hub 0.17.0, and so does the `notice` trigger. Deploy the hub first: an older hub cannot read a
`Notice` and drops it.
