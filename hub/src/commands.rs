//! Commands sent to nodes and the wait for their `Ack` or `Error`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::bail;
use oxihome_protocol::{CommandOp, Message, Payload};
use tokio::sync::oneshot;

use crate::mqtt::Mqtt;

const TIMEOUT: Duration = Duration::from_secs(5);

pub struct PendingCommand {
    answer: oneshot::Sender<Payload>,
    pub device_id: String,
    pub entity: u8,
    pub channel: u8,
    pub op: CommandOp,
}

impl PendingCommand {
    pub fn answer(self, payload: Payload) {
        let _ = self.answer.send(payload);
    }
}

/// Commands waiting for their answer, by device id and message id.
#[derive(Default)]
pub struct Commands {
    pending: Mutex<HashMap<(String, u32), PendingCommand>>,
}

impl Commands {
    /// The pending command `message` answers, if any; only the node the command went to answers it.
    pub fn answered_by(&self, message: &Message) -> Option<PendingCommand> {
        let reply_to = message.reply_to?;
        self.pending
            .lock()
            .unwrap()
            .remove(&(message.src.clone(), reply_to))
    }

    /// Sends a command and waits for the node's answer.
    pub async fn send(
        &self,
        mqtt: &Mqtt,
        device_id: &str,
        entity: u8,
        channel: u8,
        op: CommandOp,
    ) -> anyhow::Result<()> {
        let message_id = mqtt.next_message_id();
        let (answer, receiver) = oneshot::channel();
        let key = (device_id.to_string(), message_id);
        self.pending.lock().unwrap().insert(
            key.clone(),
            PendingCommand {
                answer,
                device_id: device_id.to_string(),
                entity,
                channel,
                op: op.clone(),
            },
        );

        let payload = Payload::Command {
            entity,
            channel,
            op,
        };
        if let Err(error) = mqtt.publish(message_id, Some(device_id), payload).await {
            self.pending.lock().unwrap().remove(&key);
            return Err(error);
        }

        match tokio::time::timeout(TIMEOUT, receiver).await {
            Ok(Ok(Payload::Ack { ok: true })) => Ok(()),
            Ok(Ok(Payload::Error { code, detail })) => {
                bail!(
                    "{device_id} refused the command: error {code} {}",
                    detail.unwrap_or_default()
                )
            }
            Ok(Ok(other)) => bail!("unexpected answer from {device_id}: {other:?}"),
            Ok(Err(_)) => bail!("command to {device_id} was dropped"),
            Err(_) => {
                self.pending.lock().unwrap().remove(&key);
                bail!("{device_id} did not answer within {} s", TIMEOUT.as_secs())
            }
        }
    }
}
