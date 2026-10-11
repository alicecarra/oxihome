use std::path::Path;

use anyhow::Context;
use serde::de::DeserializeOwned;

use crate::names::Names;
use crate::rules::Rules;

/// The home's configuration: the files in the config directory.
pub struct HomeConfig {
    pub names: Names,
    pub rules: Rules,
}

impl HomeConfig {
    /// A missing file counts as empty.
    pub fn load(directory: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            names: read(&directory.join("names.toml"))?,
            rules: read(&directory.join("rules.toml"))?,
        })
    }
}

fn read<T: DeserializeOwned + Default>(path: &Path) -> anyhow::Result<T> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(T::default()),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    toml::from_str(&text).with_context(|| format!("{}", path.display()))
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
