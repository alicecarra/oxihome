//! Looking up rules and dashboards, and the last run of each rule.

use std::collections::HashMap;

use jiff::civil::DateTime;

use super::{Change, Hub, ReceivedCode};
use crate::config::dashboard::Dashboard;
use crate::config::rules::{Cooldowns, Holds, Rule, RuleRun, RuleUse, Running};
use crate::node::events::EventRecord;
use crate::node::state::ChannelUpdate;
use crate::storage::Write;
use crate::traffic::Alert;

impl Hub {
    pub fn matching_rules(&self, event: &EventRecord) -> Vec<Rule> {
        let rules = self.rules.lock().unwrap();
        let remotes = self.remotes.lock().unwrap();
        rules
            .matching(event, &remotes)
            .into_iter()
            .cloned()
            .collect()
    }

    /// The dashboard of that name, or the first one, which `/` shows.
    pub fn dashboard(&self, name: Option<&str>) -> Option<Dashboard> {
        let dashboards = self.dashboards.lock().unwrap();
        match name {
            Some(name) => dashboards.named(name).cloned(),
            None => dashboards.first().cloned(),
        }
    }

    /// Every dashboard, one per tab, in file order.
    pub fn dashboards(&self) -> Vec<Dashboard> {
        self.dashboards.lock().unwrap().all().to_vec()
    }

    pub fn rules_involving(&self, device_id: &str) -> Vec<RuleUse> {
        self.rules.lock().unwrap().involving(device_id)
    }

    pub fn button_rule(&self, button: &str) -> Option<Rule> {
        self.rules.lock().unwrap().button(button).cloned()
    }

    pub fn recent_codes(&self) -> Vec<ReceivedCode> {
        self.recent_codes.lock().unwrap().iter().cloned().collect()
    }

    pub fn crossed_rules(&self, update: &ChannelUpdate) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .crossed(update)
            .into_iter()
            .cloned()
            .collect()
    }

    /// Ends the `for` waits the update or the node going offline (`None`) breaks.
    pub fn end_holds(&self, device_id: &str, update: Option<&ChannelUpdate>) {
        let names: Vec<String> = {
            let rules = self.rules.lock().unwrap();
            match update {
                Some(update) => rules.released(update),
                None => rules.waiting_on(device_id),
            }
            .into_iter()
            .map(str::to_string)
            .collect()
        };
        for name in names {
            self.holds.end(&name, device_id);
        }
    }

    /// Ends the waits of offline rules with `for` about the node, which is back.
    pub fn end_offline_holds(&self, device_id: &str) {
        let names: Vec<String> = self
            .rules
            .lock()
            .unwrap()
            .waiting_for_offline(device_id)
            .into_iter()
            .map(str::to_string)
            .collect();
        for name in names {
            self.holds.end(&name, device_id);
        }
    }

    pub fn offline_rules(&self, device_id: &str) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .offline(device_id)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn traffic_rules(&self, alert: &Alert) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .traffic(alert)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn notice_rules(&self, device_id: &str) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .notice(device_id)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn due_rules(&self, minute: DateTime) -> Vec<Rule> {
        self.rules
            .lock()
            .unwrap()
            .due(minute)
            .into_iter()
            .cloned()
            .collect()
    }

    pub fn rules(&self) -> Vec<Rule> {
        self.rules.lock().unwrap().all().to_vec()
    }

    pub fn record_rule_run(&self, name: &str, run: RuleRun) {
        self.storage
            .write(Write::RuleRun(name.to_string(), run.clone()));
        self.rule_runs.lock().unwrap().insert(name.to_string(), run);
        self.announce(Change::Timeline);
    }

    pub fn running(&self) -> &Running {
        &self.running
    }

    pub fn holds(&self) -> &Holds {
        &self.holds
    }

    pub fn cooldowns(&self) -> &Cooldowns {
        &self.cooldowns
    }

    pub fn rule_runs(&self) -> HashMap<String, RuleRun> {
        self.rule_runs.lock().unwrap().clone()
    }
}
