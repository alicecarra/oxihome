use std::collections::HashSet;

use anyhow::bail;
use serde::Deserialize;

/// Panels of buttons for the `/panel` page, from `panels.toml`; rules say what each button does
/// with `when = { panel = "...", button = "..." }`.
///
/// ```toml
/// [[panels]]
/// name = "Test board"
/// buttons = ["LED", "Alarm"]
/// ```
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panels {
    #[serde(default)]
    panels: Vec<Panel>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub name: String,
    #[serde(default)]
    pub buttons: Vec<String>,
}

impl Panels {
    /// In file order.
    pub fn all(&self) -> &[Panel] {
        &self.panels
    }

    pub fn has(&self, panel: &str, button: &str) -> bool {
        self.panels
            .iter()
            .any(|known| known.name == panel && known.buttons.iter().any(|known| known == button))
    }

    /// Names must be unique: rules refer to panels and buttons by name.
    pub fn validate(&self) -> anyhow::Result<()> {
        let mut panels = HashSet::new();
        for panel in &self.panels {
            if !panels.insert(&panel.name) {
                bail!("two panels are named \"{}\"", panel.name);
            }
            let mut buttons = HashSet::new();
            for button in &panel.buttons {
                if !buttons.insert(button) {
                    bail!("panel \"{}\" has two buttons \"{button}\"", panel.name);
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
        [[panels]]
        name = "Test board"
        buttons = ["LED", "Alarm"]

        [[panels]]
        name = "Living room"
        buttons = ["Good night"]
    "#;

    #[test]
    fn panels_keep_the_file_order() {
        let panels: Panels = toml::from_str(EXAMPLE).unwrap();
        let names: Vec<&str> = panels
            .all()
            .iter()
            .map(|panel| panel.name.as_str())
            .collect();
        assert_eq!(names, ["Test board", "Living room"]);
        assert!(panels.has("Test board", "Alarm"));
        assert!(!panels.has("Living room", "Alarm"));
        assert!(panels.validate().is_ok());
    }

    #[test]
    fn duplicate_names_and_unknown_keys_are_refused() {
        let twice: Panels =
            toml::from_str("[[panels]]\nname = \"A\"\n[[panels]]\nname = \"A\"").unwrap();
        assert!(
            twice
                .validate()
                .unwrap_err()
                .to_string()
                .contains("two panels")
        );
        let buttons: Panels =
            toml::from_str("[[panels]]\nname = \"A\"\nbuttons = [\"x\", \"x\"]").unwrap();
        assert!(
            buttons
                .validate()
                .unwrap_err()
                .to_string()
                .contains("two buttons")
        );
        assert!(toml::from_str::<Panels>("[[panels]]\nname = \"A\"\nbutton = [\"x\"]").is_err());
    }
}
