# RSS node

`rss/` (`oxihome-rss`) is a software node that follows RSS, Atom and JSON feeds (read with
`feed-rs`): an event for each new item and an unread count per feed, kept across restarts. The plan
and its decisions are in the Trilium note "Nó de RSS".

```sh
cp rss/rss.toml.example rss/rss.toml   # then the broker password and the feeds
chmod 600 rss/rss.toml
infra/mosquitto/set-password.sh rss    # the password goes in [mqtt] password
cargo run -p oxihome-rss -- rss/rss.toml
```

`rss/rss.toml` stays out of git because it holds the broker password. Each `[[feeds]]` has a fixed
`entity` from 2 up (the hub keeps history, rules and its own names by number, so removing a feed
must not renumber the others), a `name` (the entity's name on the hub, and in the log), the `url`, an `interval` in minutes (30 when missing, at least 15),
an optional `filter`, a regular expression on the title that ignores case, and an optional
`exclude`, one on the title and the link whose matching items are skipped. GitHub releases are
at `<repository>/releases.atom`, crates.io versions at
`https://static.crates.io/rss/crates/<crate>.xml`, a YouTube channel at
`https://www.youtube.com/feeds/videos.xml?channel_id=<id>`, a subreddit at
`https://www.reddit.com/r/<name>/.rss`. The `url` may also be a page that links its feed with
`<link rel="alternate">` (RSS, Atom or JSON Feed), such as a YouTube channel
(`https://www.youtube.com/@<handle>`) or a blog's home: when it is not a feed, the node follows the
first feed it links, looked up again at each start.

Entities: 1 all feeds, 0 "mark all as read" (`Trigger` without value), 1 the total unread
(`Int`). Each feed: 0 the newest item's title (`Text`), 1 its link (`Text`), 2 unread (`Int`), 3
"mark as read" (`Trigger`), 4 whether the feed answers (`Bool`, false after 3 failures in a row), 5
new item (`Event` of `Text`, `title — link`, `Received`). An example rule:

```toml
[[rules]]
name = "Beep on a new Mosquitto release"
when = { device = "rss", entity = 4, channel = 5, event = "Received" }
then = [{ invoke = { device = "esp32-08d1f9ca41e4", entity = 5, channel = 0, value = "Short" } }]
```

On the hub, the dashboard's `feeds` block (see [Dashboard](../hub/dashboard.md)) reads the node: unread
items from every feed in one list, with "Mark all read".

The first fetch of a feed (and of a feed whose URL changed) only marks its items as seen, so
nothing floods in. Afterwards, new items that pass the filters count as unread and become events,
oldest first, at most 10 per fetch (the rest only count). Opening a link does not mark anything as
read; only the two `Trigger`s do. `<name>-state.json`, next to `<name>.toml`, keeps the seen
ids (the last fetch's plus up to 500 older ones), the unread counts and the newest title and link;
it is written through a temporary file, so the directory must be writable. Fetches use `ETag` and
`If-Modified-Since` (kept in memory only), a 30 s timeout and a 5 MB limit; the feeds start 10 s
apart.

## YouTube

The YouTube channels run in a second instance of the same program, node `youtube`, so they have
their own unread count, "Mark all read" and `feeds` block (`device = "youtube"`), apart from the
blogs. Its configuration is `rss/youtube.toml`, also out of git, and its state
`rss/youtube-state.json`:

```sh
cp rss/youtube.toml.example rss/youtube.toml   # then the broker password and the channels
chmod 600 rss/youtube.toml
infra/mosquitto/set-password.sh youtube        # the password goes in [mqtt] password
cargo run -p oxihome-rss -- rss/youtube.toml
```

A channel's feed has its 15 newest uploads: videos, Shorts and live streams. Shorts have links
with `/shorts/`, so `exclude = "/shorts/"` skips them. A live stream enters the feed when it is
scheduled, not when it starts. The server sends no `ETag` or `Last-Modified` and caches the feed
for 15 minutes.
