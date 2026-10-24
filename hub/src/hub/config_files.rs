//! The config files: reading, saving and editing them, and applying a new configuration.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use anyhow::bail;

use super::{Change, Hub};
use crate::config::nodes::NodeEdits;
use crate::config::{ConfigFile, FILES, HomeConfig, read_file, read_texts, write_file};
use crate::node::hardware;
use crate::node::parameters;
use crate::storage::Write;

impl Hub {
    /// The last run of a rule no longer in `rules.toml` (removed or renamed) goes away.
    pub(super) fn forget_runs_of_removed_rules(&self) {
        let names: HashSet<String> = self
            .rules
            .lock()
            .unwrap()
            .all()
            .iter()
            .map(|rule| rule.name.clone())
            .collect();
        let mut runs = self.rule_runs.lock().unwrap();
        let removed: Vec<String> = runs
            .keys()
            .filter(|name| !names.contains(*name))
            .cloned()
            .collect();
        runs.retain(|name, _| names.contains(name));
        drop(runs);
        if !removed.is_empty() {
            self.storage.write(Write::ForgetRuleRuns(removed));
        }
    }

    /// Rereads the config directory; on error the current configuration stays.
    pub fn reload_config(self: &Arc<Self>) -> anyhow::Result<usize> {
        let _writing = self.config_writes.lock().unwrap();
        let config = HomeConfig::load(&self.config_directory)?;
        Ok(self.apply_config(config))
    }

    /// Like `reload_config`, for the file watcher: `None` when the files are as last seen,
    /// which includes the hub's own writes.
    pub fn reload_config_if_changed(self: &Arc<Self>) -> Option<anyhow::Result<usize>> {
        let _writing = self.config_writes.lock().unwrap();
        let texts = match read_texts(&self.config_directory) {
            Ok(texts) => texts,
            Err(error) => return Some(Err(error)),
        };
        let mut seen = self.config_seen.lock().unwrap();
        if *seen == texts {
            return None;
        }
        *seen = texts.clone();
        drop(seen);
        Some(
            HomeConfig::from_texts(&self.config_directory, texts)
                .map(|config| self.apply_config(config)),
        )
    }

    pub fn config_file(&self, file: &str) -> anyhow::Result<ConfigFile> {
        read_file(&self.config_directory.join(file))
    }

    /// Validates the whole configuration with the new text, then writes and applies it.
    /// Refuses when the file changed on disk since `expected_modified` was read.
    pub fn save_config_file(
        self: &Arc<Self>,
        file: &str,
        text: &str,
        expected_modified: Option<SystemTime>,
    ) -> anyhow::Result<usize> {
        let _writing = self.config_writes.lock().unwrap();
        self.write_config_file(file, text, expected_modified)
    }

    /// Like `save_config_file`, with the text made from the current one.
    fn edit_config_file(
        self: &Arc<Self>,
        file: &str,
        edit: impl FnOnce(&str) -> anyhow::Result<String>,
    ) -> anyhow::Result<()> {
        let _writing = self.config_writes.lock().unwrap();
        let current = self.config_file(file)?;
        let text = edit(&current.text)?;
        self.write_config_file(file, &text, current.modified)?;
        Ok(())
    }

    /// Only with `config_writes` held.
    fn write_config_file(
        self: &Arc<Self>,
        file: &str,
        text: &str,
        expected_modified: Option<SystemTime>,
    ) -> anyhow::Result<usize> {
        if !FILES.contains(&file) {
            bail!("{file} is not an editable config file");
        }
        let path = self.config_directory.join(file);
        if read_file(&path)?.modified != expected_modified {
            bail!("{file} changed on disk since this page was loaded; reload the page");
        }
        let config = HomeConfig::load_with(&self.config_directory, Some((file, text)))?;
        write_file(&path, text)?;
        Ok(self.apply_config(config))
    }

    /// Returns the number of rules. Nodes get their parameters and hardware sent when they
    /// differ from the new configuration.
    fn apply_config(self: &Arc<Self>, config: HomeConfig) -> usize {
        let rule_count = config.rules.len();
        *self.config_seen.lock().unwrap() = config.texts;
        *self.nodes.lock().unwrap() = config.nodes;
        self.hardware.lock().unwrap().refused.clear();
        let before = std::mem::replace(&mut *self.rules.lock().unwrap(), config.rules);
        *self.remotes.lock().unwrap() = config.remotes;
        *self.dashboards.lock().unwrap() = config.dashboards;
        self.forget_runs_of_removed_rules();
        crate::config::rules::rearm(self.clone(), before.all());
        self.announce(Change::Config);
        self.announce(Change::Nodes);
        self.check_all_references();
        for device_id in self.device_ids() {
            tokio::spawn(parameters::apply(self.clone(), device_id.clone()));
            tokio::spawn(hardware::apply(self.clone(), device_id));
        }
        rule_count
    }

    pub fn config_directory(&self) -> &Path {
        &self.config_directory
    }

    /// Adds a key to `remotes.toml` (keeping its comments), validates and applies the configuration.
    pub fn learn_key(self: &Arc<Self>, remote: &str, key: &str, code: &str) -> anyhow::Result<()> {
        self.edit_config_file("remotes.toml", |text| {
            crate::config::remotes::with_key(text, remote, key, code)
        })
    }

    /// Fails when a rule still uses the key.
    pub fn forget_key(self: &Arc<Self>, remote: &str, key: &str) -> anyhow::Result<()> {
        self.edit_config_file("remotes.toml", |text| {
            crate::config::remotes::without_key(text, remote, key)
        })
    }

    /// Turns a rule on or off in `rules.toml` (keeping its comments), validates and applies the
    /// configuration.
    pub fn set_rule_enabled(self: &Arc<Self>, name: &str, enabled: bool) -> anyhow::Result<()> {
        self.edit_config_file("rules.toml", |text| {
            crate::config::rules::with_rule_enabled(text, name, enabled)
        })
    }

    /// Puts a rule in `rules.toml` in place of the one named `previous`, or adds it (keeping
    /// comments), validates and applies the configuration.
    pub fn save_rule(
        self: &Arc<Self>,
        previous: Option<&str>,
        rule: toml_edit::Table,
    ) -> anyhow::Result<()> {
        self.edit_config_file("rules.toml", |text| {
            crate::config::rules::with_rule_saved(text, previous, rule)
        })
    }

    /// What `save_rule` would refuse, without saving: the whole configuration is validated.
    pub fn check_rule(&self, previous: Option<&str>, rule: toml_edit::Table) -> anyhow::Result<()> {
        let current = self.config_file("rules.toml")?;
        let text = crate::config::rules::with_rule_saved(&current.text, previous, rule)?;
        HomeConfig::load_with(&self.config_directory, Some(("rules.toml", &text)))?;
        Ok(())
    }

    /// Changes a node's names and parameters in `nodes.toml` (keeping its comments), validates
    /// and applies the configuration.
    pub fn edit_node(self: &Arc<Self>, device_id: &str, edits: &NodeEdits) -> anyhow::Result<()> {
        self.edit_config_file("nodes.toml", |text| {
            crate::config::nodes::with_node_edits(text, device_id, edits)
        })
    }
}
