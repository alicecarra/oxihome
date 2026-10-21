//! Whether a dashboard needs a look, for the dot on its tab.

use super::job;
use crate::config::dashboard::{Block, BlockKind, Dashboard};
use crate::hub::Hub;
use crate::web::overview;

/// Worst last: a tab shows the worst of its blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Attention {
    None,
    /// A node the block shows is offline.
    Warning,
    /// A job or a rule failed.
    Alert,
}

pub fn dashboard_attention(hub: &Hub, dashboard: &Dashboard) -> Attention {
    dashboard
        .blocks
        .iter()
        .filter(|block| block.attention)
        .map(|block| block_attention(hub, block))
        .max()
        .unwrap_or(Attention::None)
}

/// From what the block already shows: the `nodes`, `buttons` and `events` blocks never ask.
fn block_attention(hub: &Hub, block: &Block) -> Attention {
    match block.kind {
        BlockKind::Rules if overview::failing_rules(hub) > 0 => Attention::Alert,
        BlockKind::Job if job::failed(hub, block) => Attention::Alert,
        BlockKind::Nodes | BlockKind::Buttons | BlockKind::Events | BlockKind::Rules => {
            Attention::None
        }
        _ if shown_nodes(block).any(|device| !hub.is_online(device)) => Attention::Warning,
        _ => Attention::None,
    }
}

fn shown_nodes(block: &Block) -> impl Iterator<Item = &str> {
    block
        .channels
        .iter()
        .chain(block.channel.iter())
        .map(|channel| channel.device.as_str())
        .chain(block.device.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str) -> Block {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn blocks_name_the_nodes_they_show() {
        let channels = block(
            "type = \"channels\"\nchannels = [\
             { device = \"printer\", entity = 1, channel = 0 },\
             { device = \"weather\", entity = 1, channel = 0 }]",
        );
        assert_eq!(
            shown_nodes(&channels).collect::<Vec<_>>(),
            ["printer", "weather"]
        );
        let history = block(
            "type = \"history\"\nchannel = { device = \"weather\", entity = 1, channel = 0 }",
        );
        assert_eq!(shown_nodes(&history).collect::<Vec<_>>(), ["weather"]);
        let feeds = block("type = \"feeds\"\ndevice = \"rss\"");
        assert_eq!(shown_nodes(&feeds).collect::<Vec<_>>(), ["rss"]);
    }

    #[test]
    fn the_worst_level_wins() {
        assert_eq!(
            [Attention::Warning, Attention::Alert, Attention::None]
                .into_iter()
                .max(),
            Some(Attention::Alert)
        );
    }
}
