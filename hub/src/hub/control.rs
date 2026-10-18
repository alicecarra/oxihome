//! Commands to nodes: sending a `Set` or an `Invoke`, waiting for the answer and recording it.

use anyhow::bail;
use jiff::Timestamp;
use oxihome_protocol::{CommandOp, Value};

use super::{Change, CommandOrigin, Hub};
use crate::storage::{Operation, StoredCommand, Write};

impl Hub {
    /// Sends a `Set` and waits for the node's answer.
    pub async fn set(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Value,
        origin: CommandOrigin,
    ) -> anyhow::Result<()> {
        let command = StoredCommand {
            device_id: device_id.to_string(),
            entity,
            channel,
            at: Timestamp::now(),
            operation: Operation::Set,
            value: Some(value.clone()),
            rule: None,
            error: None,
        };
        let result = self
            .commands
            .send(
                &self.mqtt,
                device_id,
                entity,
                channel,
                CommandOp::Set(value),
            )
            .await;
        self.record_command(origin, command, &result);
        result
    }

    /// Sends an `Invoke` and waits for the node's answer. The node's declaration picks `Enum` or
    /// `Text` and refuses values it does not take, without sending; an unknown node gets `Enum`.
    pub async fn invoke(
        &self,
        device_id: &str,
        entity: u8,
        channel: u8,
        value: Option<String>,
        origin: CommandOrigin,
    ) -> anyhow::Result<()> {
        let declared = self
            .registry
            .lock()
            .unwrap()
            .node(device_id)
            .map(|node| node.invoke_value(entity, channel, value.as_deref()));
        let value = match declared {
            Some(Ok(value)) => value,
            Some(Err(error)) => bail!("{device_id}: {error}"),
            None => value.map(Value::Enum),
        };
        let command = StoredCommand {
            device_id: device_id.to_string(),
            entity,
            channel,
            at: Timestamp::now(),
            operation: Operation::Invoke,
            value: value.clone(),
            rule: None,
            error: None,
        };
        let result = self
            .commands
            .send(
                &self.mqtt,
                device_id,
                entity,
                channel,
                CommandOp::Invoke(value),
            )
            .await;
        self.record_command(origin, command, &result);
        result
    }

    fn record_command(
        &self,
        origin: CommandOrigin,
        command: StoredCommand,
        result: &anyhow::Result<()>,
    ) {
        let rule = match origin {
            CommandOrigin::Web => None,
            CommandOrigin::Rule(name) => Some(name),
            CommandOrigin::Hub => return,
        };
        self.storage.write(Write::Command(StoredCommand {
            rule,
            error: result.as_ref().err().map(|error| format!("{error:#}")),
            ..command
        }));
        self.announce(Change::Timeline);
    }
}
