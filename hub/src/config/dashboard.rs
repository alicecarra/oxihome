use std::collections::HashSet;

use anyhow::bail;
use serde::Deserialize;

use super::rules::Rules;

/// The home page, from `dashboard.toml`: blocks in file order on a grid of three columns. Only
/// the first dashboard is shown for now.
///
/// ```toml
/// [[dashboards]]
/// name = "Home"
///
/// [[dashboards.blocks]]
/// type = "nodes"
///
/// [[dashboards.blocks]]
/// type = "channels"
/// title = "Weather"
/// width = 2
/// channels = [{ device = "weather", entity = 1, channel = 0 }]
///
/// [[dashboards.blocks]]
/// type = "buttons"
/// buttons = ["Good night"]
///
/// [[dashboards.blocks]]
/// type = "history"
/// channel = { device = "weather", entity = 1, channel = 0 }
/// range = "6h"
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dashboards {
    #[serde(default)]
    dashboards: Vec<Dashboard>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dashboard {
    pub name: String,
    #[serde(default)]
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Block {
    #[serde(rename = "type")]
    pub kind: BlockKind,
    pub title: Option<String>,
    /// Grid columns, 1 to 3.
    #[serde(default = "default_width")]
    pub width: u8,
    /// `channels` blocks only.
    #[serde(default)]
    pub channels: Vec<ChannelReference>,
    /// `buttons` blocks only: buttons of rules, `when = { button = "..." }`.
    #[serde(default)]
    pub buttons: Vec<String>,
    /// `value` and `history` blocks only.
    pub channel: Option<ChannelReference>,
    /// `history` blocks only: `15m`, `1h`, `6h` or `24h` (default).
    pub range: Option<String>,
    /// `events` blocks only: how many to show, 1 to 50 (default 10).
    pub limit: Option<usize>,
}

pub const HISTORY_RANGES: [&str; 4] = ["15m", "1h", "6h", "24h"];
pub const DEFAULT_EVENTS: usize = 10;
const MAX_EVENTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    /// The nodes by presence, with name and id.
    Nodes,
    /// Chosen channels, with value and controls.
    Channels,
    /// Buttons that run their rules.
    Buttons,
    /// One channel's value, large.
    Value,
    /// A chart of one channel.
    History,
    /// The next scheduled runs and the rules whose last run failed.
    Rules,
    /// The latest node events and notifications.
    Events,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelReference {
    pub device: String,
    pub entity: u8,
    pub channel: u8,
}

fn default_width() -> u8 {
    1
}

impl BlockKind {
    fn name(self) -> &'static str {
        match self {
            BlockKind::Nodes => "nodes",
            BlockKind::Channels => "channels",
            BlockKind::Buttons => "buttons",
            BlockKind::Value => "value",
            BlockKind::History => "history",
            BlockKind::Rules => "rules",
            BlockKind::Events => "events",
        }
    }
}

impl Dashboards {
    pub fn first(&self) -> Option<&Dashboard> {
        self.dashboards.first()
    }

    pub fn validate(&self, rules: &Rules) -> anyhow::Result<()> {
        let mut names = HashSet::new();
        for dashboard in &self.dashboards {
            if !names.insert(&dashboard.name) {
                bail!("two dashboards are named \"{}\"", dashboard.name);
            }
            for (index, block) in dashboard.blocks.iter().enumerate() {
                let place = format!("dashboard \"{}\", block {}", dashboard.name, index + 1);
                if !(1..=3).contains(&block.width) {
                    bail!("{place}: width must be 1, 2 or 3");
                }
                match block.kind {
                    BlockKind::Nodes | BlockKind::Rules | BlockKind::Events => {}
                    BlockKind::Value | BlockKind::History if block.channel.is_none() => {
                        bail!("{place}: a {} block needs channel", block.kind.name())
                    }
                    BlockKind::Value | BlockKind::History => {}
                    BlockKind::Channels if block.channels.is_empty() => {
                        bail!("{place}: a channels block needs channels")
                    }
                    BlockKind::Channels => {}
                    BlockKind::Buttons if block.buttons.is_empty() => {
                        bail!("{place}: a buttons block needs buttons")
                    }
                    BlockKind::Buttons => {
                        if let Some(button) = block
                            .buttons
                            .iter()
                            .find(|button| rules.button(button).is_none())
                        {
                            bail!("{place}: no rule has the button \"{button}\"")
                        }
                    }
                }
                if block.kind != BlockKind::Channels && !block.channels.is_empty() {
                    bail!("{place}: only a channels block has channels");
                }
                if block.kind != BlockKind::Buttons && !block.buttons.is_empty() {
                    bail!("{place}: only a buttons block has buttons");
                }
                if !matches!(block.kind, BlockKind::Value | BlockKind::History)
                    && block.channel.is_some()
                {
                    bail!("{place}: only value and history blocks have channel");
                }
                match &block.range {
                    Some(_) if block.kind != BlockKind::History => {
                        bail!("{place}: only a history block has range")
                    }
                    Some(range) if !HISTORY_RANGES.contains(&range.as_str()) => {
                        bail!("{place}: range must be one of {}", HISTORY_RANGES.join(", "))
                    }
                    _ => {}
                }
                match block.limit {
                    Some(_) if block.kind != BlockKind::Events => {
                        bail!("{place}: only an events block has limit")
                    }
                    Some(limit) if !(1..=MAX_EVENTS).contains(&limit) => {
                        bail!("{place}: limit must be from 1 to {MAX_EVENTS}")
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
        [[dashboards]]
        name = "Home"

        [[dashboards.blocks]]
        type = "nodes"

        [[dashboards.blocks]]
        type = "channels"
        title = "Weather"
        width = 2
        channels = [{ device = "weather", entity = 1, channel = 0 }]

        [[dashboards.blocks]]
        type = "buttons"
        buttons = ["Good night"]

        [[dashboards.blocks]]
        type = "value"
        channel = { device = "weather", entity = 1, channel = 0 }

        [[dashboards.blocks]]
        type = "history"
        channel = { device = "weather", entity = 1, channel = 0 }
        range = "6h"

        [[dashboards.blocks]]
        type = "rules"

        [[dashboards.blocks]]
        type = "events"
        limit = 5
    "#;

    fn rules() -> Rules {
        toml::from_str("[[rules]]\nname = \"Lights off\"\nwhen = { button = \"Good night\" }\nthen = []")
            .unwrap()
    }

    fn error(text: &str) -> String {
        let dashboards: Dashboards = toml::from_str(text).unwrap();
        dashboards.validate(&rules()).unwrap_err().to_string()
    }

    #[test]
    fn blocks_keep_the_file_order() {
        let dashboards: Dashboards = toml::from_str(EXAMPLE).unwrap();
        assert!(dashboards.validate(&rules()).is_ok());
        let blocks = &dashboards.first().unwrap().blocks;
        let kinds: Vec<BlockKind> = blocks.iter().map(|block| block.kind).collect();
        assert_eq!(
            kinds,
            [
                BlockKind::Nodes,
                BlockKind::Channels,
                BlockKind::Buttons,
                BlockKind::Value,
                BlockKind::History,
                BlockKind::Rules,
                BlockKind::Events,
            ]
        );
        assert_eq!(blocks[4].range.as_deref(), Some("6h"));
        assert_eq!(blocks[6].limit, Some(5));
        assert_eq!(blocks[0].width, 1);
        assert_eq!(blocks[1].width, 2);
        assert_eq!(
            blocks[1].channels,
            [ChannelReference {
                device: "weather".into(),
                entity: 1,
                channel: 0
            }]
        );
    }

    #[test]
    fn invalid_blocks_are_refused() {
        let block = |fields: &str| format!("[[dashboards]]\nname = \"Home\"\n[[dashboards.blocks]]\n{fields}");
        assert!(error(&block("type = \"nodes\"\nwidth = 4")).contains("width"));
        assert!(error(&block("type = \"channels\"")).contains("needs channels"));
        assert!(error(&block("type = \"buttons\"")).contains("needs buttons"));
        assert!(error(&block("type = \"buttons\"\nbuttons = [\"Alarm\"]")).contains("no rule has"));
        assert!(error(&block("type = \"nodes\"\nbuttons = [\"Good night\"]")).contains("only a buttons"));
        let channel = "channel = { device = \"weather\", entity = 1, channel = 0 }";
        assert!(error(&block("type = \"value\"")).contains("value block needs channel"));
        assert!(error(&block("type = \"history\"")).contains("history block needs channel"));
        assert!(error(&block(&format!("type = \"rules\"\n{channel}"))).contains("only value and history"));
        assert!(error(&block(&format!("type = \"history\"\n{channel}\nrange = \"2d\""))).contains("range must be"));
        assert!(error(&block(&format!("type = \"value\"\n{channel}\nrange = \"1h\""))).contains("only a history"));
        assert!(error(&block("type = \"events\"\nlimit = 0")).contains("limit must be"));
        assert!(error(&block("type = \"rules\"\nlimit = 5")).contains("only an events"));
        assert!(
            error("[[dashboards]]\nname = \"A\"\n[[dashboards]]\nname = \"A\"")
                .contains("two dashboards")
        );
        assert!(toml::from_str::<Dashboards>(&block("type = \"clock\"")).is_err());
        assert!(toml::from_str::<Dashboards>(&block("type = \"nodes\"\ncolor = \"red\"")).is_err());
    }
}
