# Telegram bot

`telegram/` (`oxihome-telegram`) is a software node that links the house to a Telegram chat: rules
send messages to it, and the commands typed there become events. It uses `teloxide-core` (the Bot
API client only, with `rustls`) and long polling, so nothing is open to the internet. The plan and
its decisions are in the Trilium note "Bot do Telegram".

First, in Telegram, with [@BotFather](https://t.me/BotFather): `/newbot` (a name and a user name
ending in `bot`) gives the token; `/setjoingroups`, pick the bot, `Disable`, so nobody can add it
to a group. Your numeric user id comes from [@userinfobot](https://t.me/userinfobot) (send it any
message). Send the new bot a message too: a bot cannot start a private chat. Only one program may
poll a bot (a second `getUpdates` gets error 409), so working on the notebook while production
runs needs a second bot with its own token.

```sh
cp telegram/telegram.toml.example telegram/telegram.toml   # then the token, users, chat and commands
chmod 600 telegram/telegram.toml
infra/mosquitto/set-password.sh telegram   # the password goes in [mqtt] password
cargo run -p oxihome-telegram -- telegram/telegram.toml
```

`telegram/telegram.toml` stays out of git because it holds the bot token and the broker password.
`users` are the Telegram ids allowed to command; messages from anyone else are ignored without an
answer and logged. `chat` is where the rules' messages go (your user id for the private chat). Each
`[[commands]]` becomes an entry of the bot's menu (sent with `setMyCommands` at start) and an option
of the command event; names are 1 to 32 lowercase letters, digits or `_`.

Entities: 1 message, two `Trigger` channels of `Text` (up to 4096 bytes): 0 sends to `chat` with
sound, 1 silently. 2 command, an `Event` channel of `Enum` with the declared commands: `/receiver`
becomes `Received` with `receiver`; a command with `argument = true` and text after its name sends
the text on its own channel instead, 1 for the first such command, 2 for the next (up to 256 bytes). 3 connection: 0 whether Telegram answers `getUpdates` (`Bool`),
1 the last API error (`Text`). 4 reaction, an `Event` channel of `Text`: when an allowed user adds a
reaction in `chat` to one of the last 100 messages the bot sent, `Received` with the emoji, a space
and that message's text (`😴 ⏰ Depakote`; just the text for a custom emoji). The bot keeps those
messages in memory only, so after a restart a reaction to an older one does nothing; removing a
reaction does nothing either. Example rules:

```toml
[[rules]]
name = "Receiver from Telegram"
when = { device = "telegram", entity = 2, channel = 0, event = "Received", data = "receiver" }
then = [{ invoke = { device = "esp32-14335c2e7270", entity = 3, channel = 0, value = "SONY15 30 15" } }]

[[rules]]
name = "Telegram /cena: run a scene"
when = { device = "telegram", entity = 2, channel = 1, event = "Received" }
then = [{ run_named = { name = "{value}", reply = { device = "telegram", entity = 1, channel = 0 } } }]

[[rules]]
name = "Telegram /cenas: list the scenes"
when = { device = "telegram", entity = 2, channel = 0, event = "Received", data = "cenas" }
then = [{ run_named = { reply = { device = "telegram", entity = 1, channel = 0 } } }]

[[rules]]
name = "Warn on Telegram when a node goes offline"
when = { offline = "*" }
then = [{ invoke = { device = "telegram", entity = 1, channel = 0, value = "{node} went offline" } }]

[[rules]]
name = "Warn on Telegram when a channel stops reporting"
when = { stopped = "*" }
then = [{ invoke = { device = "telegram", entity = 1, channel = 0, value = "{node} › {channel} stopped reporting: {detail}" } }]
```

A command is accepted only when it is declared, at most 10 per minute, and younger than `max_age`
(60 s by default: after an outage Telegram delivers what it kept, up to 24 h). An accepted command
gets a 👍 reaction, which means it reached the broker, not that a rule ran; with the broker down the
bot answers that the command was not delivered. Anything else from an allowed user gets the list of
commands. Messages go as plain text, one at a time; one that cannot go out (no internet) is tried
again for 5 minutes and then dropped, and Telegram's flood limit (429) is respected.
