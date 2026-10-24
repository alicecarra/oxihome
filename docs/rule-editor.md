# Rule editor

A form on the hub to create, edit and duplicate rules without writing TOML. It writes to
`hub/config/rules.toml` through the same path as the enable switch on `/rules`, which validates
the file, applies it without a reload and keeps the comments.

## Pages and routes

- `/rules` has a **New rule** button next to **Edit in Settings**. Each rule card has **Edit**
  (`/rules/edit?name=<name>`) and **Duplicate** (`/rules/new?copy=<name>`).
- `/rules/new` and `/rules/edit` show the form.
- `POST /rules/editor` redraws the whole form (`#rule-editor`, htmx `hx-trigger="change, submit"`).
  This happens on every change, so the fields always match what was chosen.
- `POST /rules/save` writes the rule. When it works, it answers with `HX-Redirect` to
  `/rules#<anchor>` and shows the toast "Rule saved". When it fails, it redraws the form with the
  error.

Deleting and reordering rules are out of scope. Use **Edit in Settings** for those.

## How it works

- **Form state.** `Draft` with `TriggerDraft`, `ConditionDraft` and `ActionDraft` keeps every field
  as text. The field names are `trigger.*`, `condition.N.*` and `action.N.*`, and days are
  `<prefix>.day.<day>`. The add and remove buttons send a `command` (`add-condition`,
  `remove-condition.N`, `add-action`, `remove-action.N`).
- **Catalog.** Built from one hub snapshot on every request (no other `Hub` call while it
  lives):
  - the channels the nodes declared in `Hello` (except the management entity), with their names
    from `nodes.toml`
  - the nodes
  - the remote keys (`remotes.toml`)

  Channels and pairs go in the option values as JSON. A button trigger is a name typed in a text
  box: the rule defines the button, and `dashboard.toml` may show it.
- **Purpose.** Filters the channels each select offers:
  - Event: event access
  - Condition (also the channel value trigger): readable with a value
  - Switch: writable Bool
  - Invoke: Trigger
- **Control.** Picks the value field from the channel's type: on/off, options, number, text or
  nothing.
- **`settle()`.** Fills in defaults after each change (event `Press`, test `above`, or `equals`
  for a channel without limits such as on/off or options, alert
  `stopped` on any node, …).
- **Optional fields.** `cooldown` on the rule (not shown for button rules, which cannot have one),
  `for` on a channel value or an offline node, and the `wait` action are text boxes with durations such as `5m`.
- **Names.** Entity and channel names come from `nodes.toml`, or else from what the node declares
  in its `Hello`, like the rest of the hub.
- **Hints next to the fields.** The channel value trigger and channel conditions show the channel's current
  value with its unit. The cron and time triggers show croner's description (the same as the
  card on `/rules`) and the next 5 runs in the hub's time zone. Each action shows the warning the
  card on `/rules` would show, like an invoke value the node does not take.
- **Preview.** Shows the TOML in the style of the file: `when` inline, and `if`/`then` with one
  item per line. `Hub::check_rule` puts the rule in the current `rules.toml` text with
  `with_rule_saved` and validates the whole configuration, exactly like saving does (unique
  names and buttons, remote keys, placeholders, the dashboard's buttons). While there is an
  error, **Save** is disabled. Under **Then** the form lists the placeholders the trigger fills,
  like `{node}`.

## Files

- `hub/src/web/rule_editor.rs`: the editor, with its tests
- `hub/src/config/rules/editing.rs`: `with_rule_saved`, which replaces a rule by name and keeps
  its comments and position, or appends it, and refuses a duplicate name. Test
  `rules_are_replaced_or_added_in_the_file`.
- `hub/src/hub/config_files.rs`: `save_rule` and `check_rule`
- `hub/src/web/rules.rs`: the New rule, Edit and Duplicate links
- `hub/src/web/mod.rs`: the pages and routes
- `hub/src/components/select.rs` and `checkbox.rs`: Topcoat components (`topcoat ui add select
  checkbox`, see `docs/setup.md`). Their `lucide:*` icons were changed to `tabler:check` and
  `tabler:chevron-down` because `build.rs` only prepares the tabler set.

## Status

- Brought back from the stash on 2026-10-19 and updated to the rule model of hub 0.12 (buttons
  without panels, traffic triggers, `for`, `cooldown`, `wait`). In hub 0.13.0.
- `cargo clippy` is clean and `cargo test` passes (218), with:
  - `rules_come_back_the_same_from_the_form`: every rule of a sample with every kind, and of
    `hub/config/rules.toml`, goes rule → form → TOML → rule and must come back equal
  - `channels_show_the_names_the_node_declares` and `time_triggers_preview_their_next_runs`
  - `the_editor_does_not_lock_itself_and_checks_names`: the editor on `Hub::for_tests`, on a
    thread with a 5 s limit, and a copy with the same name is refused
- Tried by Alice on the running hub on 2026-10-19 before the names, hints and cron preview.

## Maintenance

**Updates itself, no editor changes needed:**

- new nodes, entities and channels
- names in `nodes.toml`
- enum options
- remote keys
- new checks in the configuration (the preview runs the same validation as saving)

**Needs changes in `rule_editor.rs`:**

- **A new trigger, condition or action kind** in `config/rules/`: add it to the `*Kind` enum,
  the draft, `from_rule`/`from_condition`/`from_action`, `from_fields`, `*_table` and the view.
  The `match` in the conversion from a rule fails to compile until that is done.
- **A new optional field on an existing kind.** Nothing fails to compile, but
  `rules_come_back_the_same_from_the_form` fails once the field is in the sample or in
  `hub/config/rules.toml`. Add it to `EVERY_KIND` in that test when adding the field.
- **A new `EventKind`** in the protocol: `event_label` fails to compile; add it there and to
  `EVENTS`.
- **A new `AlertKind`**: `alert_texts` fails to compile; add it there and to `ALERTS`.
- **A new `ValueType` or `Access`** may need changes in `Purpose::fits` and `Control`.

**Known limits:** days are saved in week order; leading and trailing spaces in texts are
trimmed; an empty invoke value is dropped; a condition on a channel the hub does not know reads
`true`, `false` and numbers as such.

## Ideas

- Suggest the texts a channel has already sent (for example "online"/"offline") in the Data field
  of an event trigger.
- Ctrl+S and a warning when leaving with unsaved changes, like the config editor in Settings.
- When editing, a link to the rule's History and its last run, like the card on `/rules`.
