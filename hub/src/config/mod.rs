pub mod dashboard;
pub mod nodes;
pub mod panels;
pub mod remotes;
pub mod rules;

use std::path::Path;
use std::time::SystemTime;

use anyhow::Context;
use serde::de::DeserializeOwned;

use dashboard::Dashboards;
use nodes::Nodes;
use panels::Panels;
use remotes::Remotes;
use rules::Rules;

/// The home's configuration: the files in the config directory.
pub struct HomeConfig {
    pub nodes: Nodes,
    pub rules: Rules,
    pub remotes: Remotes,
    pub panels: Panels,
    pub dashboards: Dashboards,
}

/// The files the web interface can edit.
pub const FILES: [&str; 5] = [
    "nodes.toml",
    "rules.toml",
    "remotes.toml",
    "panels.toml",
    "dashboard.toml",
];

/// A config file's text and when it was last modified (`None` if it does not exist).
pub struct ConfigFile {
    pub text: String,
    pub modified: Option<SystemTime>,
}

impl HomeConfig {
    /// A missing file counts as empty.
    pub fn load(directory: &Path) -> anyhow::Result<Self> {
        Self::load_with(directory, None)
    }

    /// Like `load`, with one file's text replaced: validates an edit before saving it.
    pub fn load_with(directory: &Path, replacement: Option<(&str, &str)>) -> anyhow::Result<Self> {
        let text = |file: &str| -> anyhow::Result<String> {
            match replacement {
                Some((replaced, text)) if replaced == file => Ok(text.to_string()),
                _ => Ok(read_file(&directory.join(file))?.text),
            }
        };
        let path = directory.join("nodes.toml");
        let nodes: Nodes = parse(&path, &text("nodes.toml")?)?;
        nodes
            .validate()
            .with_context(|| format!("{}", path.display()))?;
        let path = directory.join("remotes.toml");
        let remotes: Remotes = parse(&path, &text("remotes.toml")?)?;
        remotes
            .validate()
            .with_context(|| format!("{}", path.display()))?;
        let path = directory.join("rules.toml");
        let rules: Rules = parse(&path, &text("rules.toml")?)?;
        let panels_path = directory.join("panels.toml");
        let panels: Panels = parse(&panels_path, &text("panels.toml")?)?;
        panels
            .validate()
            .with_context(|| format!("{}", panels_path.display()))?;
        rules
            .check_remote_keys(&remotes)
            .and_then(|()| rules.check_panel_buttons(&panels))
            .with_context(|| format!("{}", path.display()))?;
        let path = directory.join("dashboard.toml");
        let dashboards: Dashboards = parse(&path, &text("dashboard.toml")?)?;
        dashboards
            .validate(&panels)
            .with_context(|| format!("{}", path.display()))?;
        Ok(Self {
            nodes,
            rules,
            remotes,
            panels,
            dashboards,
        })
    }
}

/// A missing file reads as empty.
pub fn read_file(path: &Path) -> anyhow::Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(ConfigFile {
            text,
            modified: std::fs::metadata(path)?.modified().ok(),
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile {
            text: String::new(),
            modified: None,
        }),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes through a temporary file and a rename, so a crash never leaves half a file.
pub fn write_file(path: &Path, text: &str) -> anyhow::Result<()> {
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display()))
}

fn parse<T: DeserializeOwned>(path: &Path, text: &str) -> anyhow::Result<T> {
    toml::from_str(text).with_context(|| format!("{}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("oxihome-config-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn missing_files_are_empty() {
        let directory = directory("missing");
        let config = HomeConfig::load(&directory).unwrap();
        assert_eq!(config.rules.len(), 0);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn replacement_is_validated_instead_of_the_file() {
        let directory = directory("replacement");
        std::fs::write(directory.join("rules.toml"), "").unwrap();
        let rules = r#"
            [[rules]]
            name = "Test"
            when = { device = "sim", entity = 1, channel = 0, event = "Press" }
            then = []
        "#;

        let config = HomeConfig::load_with(&directory, Some(("rules.toml", rules))).unwrap();
        assert_eq!(config.rules.len(), 1);
        assert!(HomeConfig::load_with(&directory, Some(("rules.toml", "[[rules]]"))).is_err());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn write_file_replaces_the_content() {
        let directory = directory("write");
        let path = directory.join("nodes.toml");
        write_file(&path, "a = 1\n").unwrap();
        write_file(&path, "a = 2\n").unwrap();

        let file = read_file(&path).unwrap();
        assert_eq!(file.text, "a = 2\n");
        assert!(file.modified.is_some());
        assert!(!directory.join("nodes.toml.tmp").exists());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn nodes_toml_holds_names_and_parameters() {
        let directory = directory("nodes");
        let text = r#"
            [nodes."esp32-test"]
            name = "Test board"
            entities.1 = { name = "LED", channels.0 = "Power" }
            parameters = { heartbeat_interval = 60 }
        "#;
        let config = HomeConfig::load_with(&directory, Some(("nodes.toml", text))).unwrap();
        assert_eq!(
            config
                .nodes
                .name(&nodes::NameTarget::Node("esp32-test".into())),
            Some("Test board")
        );
        assert_eq!(config.nodes.desired("esp32-test").len(), 1);
        let misspelled = "[nodes.\"esp32-test\"]\nnmae = \"Test board\"\n";
        let error = HomeConfig::load_with(&directory, Some(("nodes.toml", misspelled)))
            .err()
            .unwrap();
        assert!(format!("{error:#}").contains("nmae"), "{error:#}");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn invalid_file_is_reported_with_its_name() {
        let directory = directory("invalid");
        std::fs::write(directory.join("rules.toml"), "[[rules]]\nname = 3\n").unwrap();

        let error = HomeConfig::load(&directory).err().unwrap();
        let message = format!("{error:#}");
        assert!(message.contains("rules.toml"), "{message}");
        assert!(message.contains("line 2"), "{message}");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
