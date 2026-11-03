# Dashboard

`dashboard.toml`: the home page `/`, a list of blocks in file order on a grid of three columns
(one on phones), refreshed live; the cards of a row share its height. Each block has a `type`, an optional `title` and a `width` of
1 to 3 columns (default 1). Types: `nodes` (nodes by presence), `channels` (chosen channels with
value and controls), `buttons` (buttons of rules by name, `buttons = ["Turn on", "Turn off"]`; a button of a disabled rule is greyed out), `room` (a room's summary, `room = "Living room"`), `value` (one channel's value, large, dimmed
while its node is offline), `history` (the node page's chart for one channel, with 15 min, 1 h, 6 h and 24 h
buttons; `range` of `15m`, `1h`, `6h` or `24h` sets the default, `24h` when missing; the picked range stays
in the address, `?range4=6h` for the fifth block, through live updates and reloads), `rules` (the next 5 scheduled runs and the rules whose last run failed,
with the error) and `events` (the latest node events and notifications, newest first, `limit` 1 to
50, default 10; IR codes show the key name from `remotes.toml`) and `feeds` (the items of a node
laid out like the RSS node, `device = "rss"`: every feed in one list, newest first, the title
linking to the post in a new tab, the feed's name and age below; only unread items, with "Show
read (N)" bringing the read ones back, grey, and the choice kept in the address, `?read5=1` for
the sixth block; "Mark all read" invokes the node's entity 1; `limit` 1 to 50, default 10). The
node keeps an unread count per feed, not which items, so a feed's newest N items are the unread
ones. `reminders` (the reminders of a node laid out like the reminders node, `device =
"reminders"`): pending ones first, in red with "Pending since 08:30", then the others by due time;
each with **Done** and, while pending, Snooze buttons (15 min, 1 h, 3 h, tomorrow) that invoke the
node; a snoozed one shows "Snoozed until". Notifications are stored in the database too (table `notifications`, same retention as
events), so they survive a restart.

Each `[[dashboards]]` is a tab, in file order: the first one at `/` (where the installed app
opens), every one at `/d/<name>`; the name cannot be empty or have `/`. With a single dashboard
there are no tabs. The tab row stays under the header while scrolling; a tab swaps the page in
place and back works. A dot on another tab says it needs a look: red when a `job` block's state
is `failed` or `error` or a `rules` block has a rule whose last run failed, amber when a node a
`channels`, `value`, `history`, `feeds`, `reminders` or `job` block shows is offline or a
`reminders` block has a pending reminder that is not snoozed. `attention = false` on a
block leaves it out.

```toml
[[dashboards]]
name = "Home"

[[dashboards.blocks]]
type = "value"
channel = { device = "esp32-08d1f9ca41e4", entity = 3, channel = 0 }

[[dashboards.blocks]]
type = "history"
width = 2
channel = { device = "esp32-08d1f9ca41e4", entity = 3, channel = 0 }
range = "6h"

[[dashboards.blocks]]
type = "events"
limit = 5

[[dashboards.blocks]]
type = "feeds"
device = "rss"
width = 2
```
