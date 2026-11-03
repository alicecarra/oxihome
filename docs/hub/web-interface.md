# Web interface

Clicking a node's name on the home page opens its page, `/nodes/<id>`, with two sections that
update live. **Node overview**: name, presence, device id, firmware, uptime, signal and the
parameters (the node's value in red when it differs from the desired one). **Node fields**: a grid
of cards, one per entity, each channel with what it is (type, range and unit, options, access), its
value or last event and its controls. A number the node measures (a read only `Int` or `Float`,
such as temperature or humidity) also gets a chart from the database (see [Database](database.md)). Buttons next to **Edit names** pick the last
15 min, 1 h (default), 6 h or 24 h for every chart of the page; the choice stays in the address
(`?range=6h`), so live updates, edits and reloads keep it. A chart has 144 steps (about 6 s, 25 s, 2.5 min
or 10 min), the line at each step's average and a light band from its minimum
to its maximum, with the overall minimum and maximum below. Values are stored only when they change,
so a step without readings keeps the previous value; the chart starts at the first value known.
It is inline SVG drawn by the hub (`hub/src/web/chart.rs`), without JavaScript. Each section has its own edit button (**Edit** for the node
name and parameters, **Edit names** for entity and channel names), which turns only that section
into fields; it stops updating while edited. Saving writes `nodes.toml` with `toml_edit`, keeping
comments and the `entities.1 = { ... }` lines: an empty field removes the name or parameter, a field
left out stays as it is (an entity or channel name field holds only the hub's own name, with the
node's name as placeholder, so emptying it goes back to the node's name), the whole configuration is validated first (an error shows at the top and
keeps what was typed), and changed parameters go to the node right away. A node that never sent
a `Hello` (none in the database either) shows as unknown. At the bottom, **Rules** lists, one line per rule, the rules
that run on something from the node (an event or value of one of its channels, the node going
offline, `offline = "*"` included), the channels or presence their conditions check and the channels they act on; each name links to the rule's card
on `/rules`, which is highlighted. A channel used by rules shows a "N rules" mark (their names on
hover) that jumps there. Remote keys, buttons and schedules are not tied to a node, so those
rules only show through their actions. The pages set the mobile viewport, so they fit a
phone screen.

The page uses htmx: the switches post to `/set` and the controls of `Trigger` channels (a button per
option, such as the buzzer patterns, or a text box for a `Text` channel) post to `/invoke`, without
reloading the page, and the hub announces
changes over server-sent events at `/events` (`nodes` when a value, event or presence changes,
`notifications` for a new toast). The page fetches the node list and the toasts again when it hears
them, and every 30 s as well, which also keeps the "3 s ago" labels moving. To watch the events:
`curl -N localhost:8080/events`.

`/set` answers with an empty body on success or the error message; a missing `value` means `false`
(an unchecked switch sends nothing). The same
request from the command line:

```sh
curl -X POST -d "device_id=esp32-08d1f9ca41e4&entity=1&channel=0&value=true" localhost:8080/set
curl -X POST -d "device_id=esp32-08d1f9ca41e4&entity=5&channel=0&value=Short" localhost:8080/invoke
```

These `curl` lines, and the `/events` one above, work as they are only on a hub without
`[http] password_hash`; with the login on, add the session cookie copied from the browser (`-b`).

Styles use Tailwind CSS 4. `hub/build.rs` runs Topcoat's Tailwind integration: on the first build it
downloads the standalone Tailwind CLI (pinned by Topcoat, cached in `target/topcoat/cache/tailwind/`,
needs internet once), scans `hub/src/**/*.rs` for class names (input: `hub/styles.css`) and writes
the CSS that the hub serves at `/static/app.css`. No Node and no `topcoat asset bundle` are needed.
Class names must appear literally in the code; names assembled at runtime are not generated.

UI components come from Topcoat UI (shadcn/ui style): their Rust source is copied into
`hub/src/components/` and becomes ours to edit; the theme (design tokens, light and dark) is
`hub/styles.css` and the install state is `hub/components.toml`. The Topcoat CLI is only needed to
add or update components, not to build or run the hub:

```sh
cargo install topcoat-cli --version 0.10.0 --locked
cd hub
topcoat ui list                  # available and installed components
topcoat ui add badge button card # copy components into src/components
topcoat ui add select checkbox   # for the rule editor
```

Avoid `dialog`, `alert_dialog`, `sheet`, `sidebar` and `tabs`: they need Topcoat's JavaScript
runtime, which the hub does not use.

htmx and its SSE extension are vendored in `hub/static/` and served by the hub, so the page works
without internet access. To update them, change the version and download again:

```sh
curl -sSfL -o hub/static/htmx.min.js https://cdn.jsdelivr.net/npm/htmx.org@2.0.10/dist/htmx.min.js
curl -sSfL -o hub/static/htmx-ext-sse.min.js https://cdn.jsdelivr.net/npm/htmx-ext-sse@2.2.4/dist/sse.min.js
```
