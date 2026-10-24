//! The Telegram side: long polling for commands and reactions, and the queue of outgoing messages.

use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oxihome_node::mqtt::Handle;
use serde::Deserialize;
use teloxide_core::RequestError;
use teloxide_core::payloads::{GetUpdatesSetters, SendMessageSetters, SetMessageReactionSetters};
use teloxide_core::prelude::*;
use teloxide_core::types::{
    AllowedUpdate, BotCommand, ChatId, Message, MessageReactionUpdated, ReactionType, UpdateKind,
    UserId,
};
use tokio::sync::mpsc;

use crate::node::{Outgoing, TelegramNode};

const POLL_TIMEOUT_S: u32 = 50;
const RETRY_DELAY: Duration = Duration::from_secs(5);
/// A message that could not go out by then is dropped, so a long outage does not end in a flood.
const SEND_GIVE_UP: Duration = Duration::from_secs(5 * 60);
const COMMANDS_PER_MINUTE: usize = 10;
const ACCEPTED_REACTION: &str = "👍";

#[derive(Deserialize)]
pub struct Command {
    pub name: String,
    pub description: String,
}

impl Command {
    pub fn check(&self) -> Result<(), String> {
        let name = &self.name;
        if name.is_empty()
            || name.len() > 32
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(format!(
                "command \"{name}\": 1 to 32 lowercase letters, digits or _"
            ));
        }
        let length = self.description.chars().count();
        if !(1..=256).contains(&length) {
            return Err(format!(
                "command \"{name}\": description of 1 to 256 characters"
            ));
        }
        Ok(())
    }
}

pub struct Settings {
    pub users: Vec<UserId>,
    /// Where messages go; reactions count only there.
    pub chat: ChatId,
    pub max_age: Duration,
    pub commands: Vec<Command>,
}

/// Runs until the node stops.
pub async fn poll(bot: Bot, settings: Settings, handle: Handle<TelegramNode>) {
    let menu: Vec<BotCommand> = settings
        .commands
        .iter()
        .map(|command| BotCommand::new(&command.name, &command.description))
        .collect();
    let mut menu_set = false;
    let mut offset = 0;
    let mut reachable = None;
    let mut limiter = RateLimiter::default();
    loop {
        if !menu_set {
            match bot.set_my_commands(menu.clone()).await {
                Ok(_) => menu_set = true,
                Err(error) => println!("setting the command menu failed: {error}"),
            }
        }
        let result = bot
            .get_updates()
            .offset(offset)
            .timeout(POLL_TIMEOUT_S)
            .allowed_updates([AllowedUpdate::Message, AllowedUpdate::MessageReaction])
            .await;
        let now_reachable = result.is_ok();
        if reachable != Some(now_reachable) {
            reachable = Some(now_reachable);
            if !handle.act(move |node| node.reachable(now_reachable)).await {
                return;
            }
        }
        match result {
            Ok(updates) => {
                for update in updates {
                    offset = update.id.0 as i32 + 1;
                    let running = match update.kind {
                        UpdateKind::Message(message) => {
                            answer(&bot, &settings, &handle, &mut limiter, &message).await
                        }
                        UpdateKind::MessageReaction(reaction) => {
                            react(&bot, &settings, &handle, &mut limiter, &reaction).await
                        }
                        _ => true,
                    };
                    if !running {
                        return;
                    }
                }
            }
            Err(error) => {
                println!("getUpdates failed: {error}");
                let delay = match &error {
                    RequestError::RetryAfter(seconds) => seconds.duration(),
                    _ => RETRY_DELAY,
                };
                let text = error.to_string();
                if !handle.act(move |node| node.failed(text)).await {
                    return;
                }
                tokio::time::sleep(delay).await;
            }
        }
    }
}

/// `false` when the node stopped.
async fn answer(
    bot: &Bot,
    settings: &Settings,
    handle: &Handle<TelegramNode>,
    limiter: &mut RateLimiter,
    message: &Message,
) -> bool {
    let Some(user) = &message.from else {
        return true;
    };
    if !settings.users.contains(&user.id) {
        println!(
            "ignored a message from user {} ({})",
            user.id,
            user.username.as_deref().unwrap_or(&user.first_name)
        );
        return true;
    }
    let reply = |text: String| async move {
        if let Err(error) = bot.send_message(message.chat.id, text).await {
            println!("replying failed: {error}");
        }
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    if now - message.date.timestamp() > settings.max_age.as_secs() as i64 {
        reply("Old command, ignored.".into()).await;
        return true;
    }
    let declared = message.text().and_then(command_name).filter(|name| {
        settings
            .commands
            .iter()
            .any(|command| command.name == *name)
    });
    let Some(name) = declared else {
        reply(help(&settings.commands)).await;
        return true;
    };
    if !limiter.allow(Instant::now()) {
        reply("Too many commands, wait a minute.".into()).await;
        return true;
    }
    if !handle.is_connected() {
        reply("No connection to the house, command not delivered.".into()).await;
        return true;
    }
    let name = name.to_string();
    if !handle.act(move |node| node.command_received(name)).await {
        return false;
    }
    let reaction = ReactionType::Emoji {
        emoji: ACCEPTED_REACTION.into(),
    };
    if let Err(error) = bot
        .set_message_reaction(message.chat.id, message.id)
        .reaction([reaction])
        .await
    {
        println!("reacting failed: {error}");
    }
    true
}

/// A reaction added by an allowed user to a message the bot sent. `false` when the node stopped.
async fn react(
    bot: &Bot,
    settings: &Settings,
    handle: &Handle<TelegramNode>,
    limiter: &mut RateLimiter,
    reaction: &MessageReactionUpdated,
) -> bool {
    let added = reaction
        .new_reaction
        .iter()
        .find(|new| !reaction.old_reaction.contains(new));
    let allowed = reaction
        .user()
        .is_some_and(|user| settings.users.contains(&user.id));
    let Some(added) = added.filter(|_| reaction.chat.id == settings.chat && allowed) else {
        return true;
    };
    let emoji = match added {
        ReactionType::Emoji { emoji } => Some(emoji.clone()),
        _ => None,
    };
    if !limiter.allow(Instant::now()) {
        return true;
    }
    if !handle.is_connected() {
        let text = "No connection to the house, reaction not delivered.";
        if let Err(error) = bot.send_message(settings.chat, text).await {
            println!("replying failed: {error}");
        }
        return true;
    }
    let message_id = reaction.message_id.0;
    handle
        .act(move |node| node.reacted(message_id, emoji.as_deref()))
        .await
}

/// `/receiver` or, in groups, `/receiver@the_bot`, with anything after it ignored.
fn command_name(text: &str) -> Option<&str> {
    let word = text.strip_prefix('/')?.split_whitespace().next()?;
    let name = word.split('@').next()?;
    (!name.is_empty()).then_some(name)
}

fn help(commands: &[Command]) -> String {
    let mut text = String::from("Commands:");
    for command in commands {
        text.push_str(&format!("\n/{} - {}", command.name, command.description));
    }
    text
}

#[derive(Default)]
struct RateLimiter {
    accepted: VecDeque<Instant>,
}

impl RateLimiter {
    fn allow(&mut self, now: Instant) -> bool {
        while self
            .accepted
            .front()
            .is_some_and(|at| now.duration_since(*at) >= Duration::from_secs(60))
        {
            self.accepted.pop_front();
        }
        if self.accepted.len() >= COMMANDS_PER_MINUTE {
            return false;
        }
        self.accepted.push_back(now);
        true
    }
}

/// Sends what the rules invoke, one message at a time, until the node stops.
pub async fn send(
    bot: Bot,
    chat: ChatId,
    mut outgoing: mpsc::Receiver<Outgoing>,
    handle: Handle<TelegramNode>,
) {
    while let Some(message) = outgoing.recv().await {
        loop {
            if message.queued_at.elapsed() > SEND_GIVE_UP {
                println!("dropped a message that could not be sent in time");
                break;
            }
            let result = bot
                .send_message(chat, &message.text)
                .disable_notification(message.silent)
                .await;
            let error = match result {
                Ok(sent) => {
                    let text = message.text;
                    if !handle.act(move |node| node.sent(sent.id.0, text)).await {
                        return;
                    }
                    break;
                }
                Err(error) => error,
            };
            println!("sending failed: {error}");
            let delay = match &error {
                RequestError::RetryAfter(seconds) => Some(seconds.duration()),
                RequestError::Network(_) => Some(RETRY_DELAY),
                _ => None,
            };
            let text = error.to_string();
            if !handle.act(move |node| node.failed(text)).await {
                return;
            }
            match delay {
                Some(delay) => tokio::time::sleep(delay).await,
                None => break,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(name: &str, description: &str) -> Command {
        Command {
            name: name.into(),
            description: description.into(),
        }
    }

    #[test]
    fn command_names_follow_telegram_rules() {
        assert!(command("ligar_ar2", "Liga o ar").check().is_ok());
        assert!(command("Receiver", "x").check().is_err());
        assert!(command("luz-sala", "x").check().is_err());
        assert!(command("", "x").check().is_err());
        assert!(command(&"a".repeat(33), "x").check().is_err());
        assert!(command("luz", "").check().is_err());
    }

    #[test]
    fn reads_the_command_name() {
        assert_eq!(command_name("/receiver"), Some("receiver"));
        assert_eq!(command_name("/receiver@casa_bot now"), Some("receiver"));
        assert_eq!(command_name("receiver"), None);
        assert_eq!(command_name("/"), None);
        assert_eq!(command_name("/@casa_bot"), None);
    }

    #[test]
    fn limits_commands_per_minute() {
        let mut limiter = RateLimiter::default();
        let start = Instant::now();
        for _ in 0..COMMANDS_PER_MINUTE {
            assert!(limiter.allow(start));
        }
        assert!(!limiter.allow(start + Duration::from_secs(59)));
        assert!(limiter.allow(start + Duration::from_secs(60)));
    }
}
