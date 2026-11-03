# Configuration

The home's configuration lives in TOML files in the directory set in `[config] dir` (default
`hub/config/`, versioned in git). The hub reads them at start. They can be edited on `/settings`,
where each file has a text box and a Save button: the whole configuration is validated with the
new text before the file is written, and an error (file, line, column) leaves the file untouched.
Saving is refused if the file changed on disk since the page was loaded. After editing a file in an
editor instead, press **Reload config from disk**; a file with an error is reported there and the
previous configuration stays in use.

After each `Hello` and each configuration change, the hub checks that every rule, dashboard block
and name in `nodes.toml` points at an entity and channel the node declares. When something the
config uses is gone (a node update dropped or moved it), a notification says what and where
(`weather: the config uses channels it no longer has`, `rule "Clock": 3/0`), once until the list
changes, and the rule's card on `/rules` says it in red. Nodes never heard from are not checked.
When the config is fixed, a newer notification replaces the warning. Entity and channel ids are
never reused: a node that drops one leaves the id unused.

- `nodes.toml`: each node by the id from its `Hello`, with its friendly names (node, entities,
  channels) and its desired operation parameters in seconds. A node may name its entities and
  channels in its `Hello`; a name here wins over the node's, so only names the node does not give,
  or that the house calls something else, need to be here. The hub sends a `Set` when a node
  reports another value: when it connects, comes back online, or after a reload. A parameter left
  out keeps the node's value. Ranges: `heartbeat_interval` 5–300, `wifi_check_interval` 1–60.
  The node's page shows the node's value next to the desired one. A misspelled key is an error. (Until
  2026-10-12 the names were in a separate `names.toml`.)

  ```toml
  [nodes."esp32-08d1f9ca41e4"]
  name = "Test board"
  entities.1 = { name = "LED", channels.0 = "Power" }
  parameters = { heartbeat_interval = 30, wifi_check_interval = 5 }
  ```

- Rooms, in `nodes.toml`: `rooms = ["Office", "Living room"]` above the first node section sets them
  and their order, and `room = "Living room"` in a node's section places it; a room that is not in the
  list stops the file from loading. `/rooms` lists each room with its nodes online, what they
  measure and its scenes, `/rooms/<name>` adds every channel with value and controls, and `/nodes`
  is ordered by room with a Room column. Nodes and scenes without a room stay out of `/rooms`.

- `remotes.toml`: infrared remotes and the codes of their keys. The `/remotes` page lists them and
  has a "Learn" card with the last codes received (not repeats nor `raw` signals): name a remote and
  a key and the hub adds it to the file, keeping its comments. The remote field starts with the remote
  of the last learned key. A code on two keys is an error. Node
  cards show a named code as `Kit Lafvin · 0`. Every key, and every code in "Learn", has a "Send"
  button that sends it through the IR transmitter picked at the top of the page (the transmitters
  are the `ir_transmitter` entries of `nodes.toml`; with none, the buttons are disabled). A
  "Codes" switch there shows or hides the code column of the remotes (hidden by default; the choice
  stays in the browser).

  ```toml
  [remotes."Kit Lafvin".keys]
  0 = "NEC 00 52"
  1 = "NEC 00 16"
  ```
