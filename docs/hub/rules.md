# Rules

`rules.toml`: a rule runs its actions when its trigger (`when`) fires. Actions set or toggle
boolean channels, or show a notification as a toast on the web interface
(`notify = { title = "...", message = "..." }`), or fire a `Trigger` channel such as the test
board buzzer (`invoke = { device = "...", entity = 5, channel = 0, value = "Alarm" }`) or an IR
transmitter (`value = "SONY15 30 15"`, the code to send). The node's
`Hello` decides what `value` is: one of the options of an `Enum` channel, or any text up to the
`max_length` of a `Text` channel. While the node is online the hub refuses other values without
sending them, and `/rules` shows the problem under the action. In the title and message of
`notify` and the `value` of `invoke`, placeholders take what the trigger is about:

| Trigger | Placeholders |
|---|---|
| channel event | `{node}`, `{channel}`; `{value}` too for `Received` (the event's text, e.g. `offline`) |
| channel limit (`above`/`below`) | `{node}`, `{channel}`, `{value}` (the new value with its unit, e.g. `52.3 km/h`) |
| `offline` | `{node}` |
| `stopped`, `noisy` | `{node}`, `{channel}`, `{detail}` |
| `unanswered`, `reported` | `{node}`, `{detail}` (the report's text, or e.g. "Used to report every 30 s") |
| `notice` | `{node}`, `{title}`, `{message}` |
| `at`, `cron`, `button`, `remote` | none |

Every trigger with `{node}` also has `{room}`, the node's room, or `no room` without one.

Names come from `nodes.toml`, then from what the node declares. A placeholder the trigger does
not fill, or a misspelled one, stops the file from loading; `{{` and `}}` are literal braces.
An `invoke` on an `Enum` channel cannot use placeholders, since the value must be one of the
options. "Run now" on `/rules` fills them with stand-ins ("any node", "a channel", "run by
hand", the limit for `{value}`), or the node's name when the trigger names one; `/rules` checks
an `invoke`'s text against the channel with those stand-ins.
`{ wait = "2s" }` between actions pauses the rule (`"500ms"`, `"1m 30s"`; more than zero, at
most 30 minutes), e.g. to give a receiver time to turn on before choosing its input; a hub
restart during the wait drops the rest. While a rule's actions run, the same rule firing again
about the same node and channel is ignored ("still running" in the log), so a double press
does not send the IR codes twice; another node, as with `offline = "*"`, still runs. "Run now"
on a rule with a wait answers right away and its card shows "running" until it ends.
A rule waiting out its `for` shows "waiting" and when it runs if nothing changes. Waits are
kept in the `rule_waits` table and go on after a hub restart; one whose time ran out while the
hub was down, or runs out before the node can talk again, waits until the node could have gone
offline (its heartbeat interval times `missed_heartbeats`) and runs only if the value is still so
then. A wait whose rule changed or went away, also while the hub was down, is dropped; a new or
changed rule whose value already passes starts waiting from the moment it is saved.
`{ run = "Cinema" }` runs the rule of that button, a scene, and waits for it to end, so a
remote key, a board's button and a schedule can share one list of actions; its own conditions
still apply. A `run` of a button no rule has, a circle of scenes or more than 8 levels stop the
file from loading; a disabled scene or one still running fails the action when it runs.
`{ run_named = { name = "{value}", reply = { device = "telegram", entity = 1, channel = 0 } } }`
takes the name from a text, whatever its case and accents, starts that scene without waiting for
it and answers on the `reply` channel; without `name`, or with an unknown one, it answers with the
list of scenes. A button rule cannot use it, since its name could close a circle.
`enabled = false` turns a rule off. Triggers:
- an event of a channel: `when = { device = "...", entity = 2, channel = 0, event = "Press" }`;
  `data = "NEC 00 52"` also requires the event's text to be exactly that (a held key's
  `NEC 00 52 repeat` does not match);
- a button, pressed on a `buttons` block of the dashboard or with "Press" on `/rules`:
  `when = { button = "Turn on" }` or `when = { scene = "Turn on" }`, which is the same (the text
  is the button's label; two rules with the same button are an error when the configuration
  loads); `room = "Living room"` next to `when` shows it on that room's page and must be one of the
  `rooms` of `nodes.toml` (the rule editor writes `button`);
- a key of a remote named in `remotes.toml`, from any IR receiver:
  `when = { remote = "Kit Lafvin", key = "0" }` (repeats do not match; an unknown key is an
  error when the configuration loads);
- a local time, every day or on some days: `when = { at = "07:00", days = ["monday", "friday"] }`;
- a channel value: `when = { device = "...", entity = 3, channel = 0, above = 28 }` (or
  `below = 27`, or `equals` with `true`/`false`, a number or the text of a `Text` or `Enum`
  value, as in a condition). It runs when a value confirmed by the node starts passing, not again
  while it keeps passing; the first value the hub learns counts as a start (a value kept in the
  database from before a restart is not a first value, so a restart does not run the rule again).
  A pair of rules (on above 28, off below 27) gives hysteresis. `for = "10m"` (at most 24h) runs
  it only if the value keeps passing that long: a confirmed value that fails, or the node going
  offline, ends the wait, and the value must still pass when the time is up. Door open for 10
  minutes is `{ device = "...", entity = 7, channel = 0, equals = true, for = "10m" }`;
- a cron expression, standard five fields (minute, hour, day of month, month, day of week):
  `when = { cron = "*/30 8-22 * * mon-fri" }`. When both the day of month and the day of week
  are restricted, either one matching is enough, as in classic cron;
- a node going offline: `when = { offline = "sim-bedroom" }`, or `offline = "*"` for any node.
  It runs when the hub marks the node offline (the same moment as the toast). With
  `for = "30m"` it runs only if the node is still offline that long after, each node with its own
  wait;
- a node coming back: `when = { online = "sim-bedroom" }`, or `online = "*"` for any node, alone
  in `when`. It runs when a node the hub saw go offline sends again (the same moment as the
  "back online" toast), not for the nodes reconnecting when the hub starts; `{node}` names it;
- an alert of the traffic map on `/traffic`, checked every 10 s: `when = { stopped = "..." }` (a
  channel that sent `State` regularly went quiet while the node keeps sending other messages;
  regularly means its recent `State` cover at least 2 minutes and its longer gaps are at most
  twice its shorter ones, so a channel that only reports changes, like the potentiometer, never
  stops; a node that goes silent altogether is the `offline` trigger's), `noisy = "..."` (a channel sends
  far more than usual for at least 3 of the last 5 minutes, so holding a remote key is not noise)
  or `unanswered = "..."` (commands to the node went without an answer in the last hour), with a
  node id or `"*"` for any node. It runs when the alert goes up, not again while it stays up nor
  within 30 minutes of the last time; alerts about a node that is offline or installing an
  update are skipped, also after it comes back. The hub also shows a toast. The counts live in
  memory, so they start over when the hub restarts;
- a node reporting a problem by itself: `when = { reported = "..." }` (node id or `"*"`), for an
  `Error` that answers no command, such as the firmware's "DHT11 not responding". It runs for
  every report; the hub also shows a toast and lists the report on the node's page;
- a node's `Notice`: `when = { notice = "..." }` (node id or `"*"`), for something a person
  should know that the node itself decided, such as a pending reminder. It runs for every
  notice; the hub also shows it as a toast (a notice with the same `key` replaces that node's
  earlier toast) and keeps it with the notifications.

`cooldown = "1h"` (optional, at most 168h; not on button rules) keeps a rule from starting again
that soon about the same node and channel, so `offline = "*"` still reports other nodes; the
hub logs `rule "...": skipped, cooling down`. "Run now" ignores it and does not start it, and a
hub restart forgets it.

Conditions (`if`, optional) are checked when the trigger fires; the actions only run when every
condition holds, otherwise the hub logs `rule "...": skipped, ... does not hold`. Each condition is
one of:
- the current value of a channel: `{ device = "...", entity = 3, channel = 0, above = 25 }`, or
  `below`, or `equals` (`true`/`false`, a number, or the text of a `Text` or `Enum` value). It does
  not hold while the node is offline or the value is unknown;
- a local time range: `{ from = "22:00", to = "06:00" }`, from included, to excluded; when `from`
  is later than `to` the range crosses midnight;
- days of the week: `{ days = ["saturday", "sunday"] }`;
- a node online or offline: `{ online = "sim-bedroom" }`, `{ offline = "sim-bedroom" }`.

```toml
[[rules]]
name = "Button at night lights the lamp only when it is off"
when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
if = [
    { from = "22:00", to = "06:00" },
    { device = "sim-bedroom", entity = 1, channel = 0, equals = false },
]
then = [{ set = { device = "sim-bedroom", entity = 1, channel = 0, value = true } }]
```

Times are local, in the zone set in `[time] zone` of `hub/hub.toml` (an IANA name such as
`America/Sao_Paulo`; required; the hub reads the machine's tzdata and falls back to a copy built
into it, so the machine's own zone does not matter). Scheduled rules are checked once per minute:
the minute the hub starts does not run, minutes skipped by a short gap (up to 5 minutes, e.g. a
clock adjustment) run right after, and longer gaps (hub stopped or suspended) are not run late.
`/rules` lists the rules with their trigger, conditions, next run and last result. Examples:

```toml
[[rules]]
name = "Living room button toggles bedroom lamp"
when = { device = "sim-living-room", entity = 2, channel = 0, event = "Press" }
then = [{ toggle = { device = "sim-bedroom", entity = 1, channel = 0 } }]

[[rules]]
name = "Test board LED on for waking up"
when = { at = "07:00", days = ["monday", "tuesday", "wednesday", "thursday", "friday"] }
then = [{ set = { device = "esp32-08d1f9ca41e4", entity = 1, channel = 0, value = true } }]

[[rules]]
name = "Test board LED blinks: on at even minutes"
when = { cron = "*/2 * * * *" }
then = [{ set = { device = "esp32-08d1f9ca41e4", entity = 1, channel = 0, value = true } }]
```
