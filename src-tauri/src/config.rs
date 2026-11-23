//! App settings in ~/.budget4/config.json (shared with the earlier Node version, so this machine
//! keeps the same sync device GUID).

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use budget4_core::importing::CsvMapping;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    /// This installation's sync device GUID (YNAB 4 also uses one GUID per machine).
    #[serde(rename = "deviceGUID", default)]
    pub device_guid: String,
    #[serde(default)]
    pub friendly_name: String,
    #[serde(default)]
    pub recent_budgets: Vec<String>,
    /// Enter due scheduled transactions on open and when the date changes (YNAB 4 does).
    #[serde(default = "yes")]
    pub enter_scheduled_transactions: bool,
    /// Write a .y4backup before the first save of each session.
    #[serde(default = "yes")]
    pub backup_on_first_save: bool,
    /// CSV column layout last used for each account (by account id).
    #[serde(default)]
    pub csv_mappings: HashMap<String, CsvMapping>,
    /// Argon2 hash of the app PIN (see lock.rs); absent when there is none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pin_hash: Option<String>,
}

fn yes() -> bool {
    true
}

pub fn config_dir() -> PathBuf {
    std::env::var_os("BUDGET4_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".budget4"))
}

fn config_file() -> PathBuf {
    config_dir().join("config.json")
}

pub fn hostname() -> String {
    std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).unwrap_or_else(|_| "Budget4".into())
}

impl AppConfig {
    pub fn load() -> Self {
        let mut c: AppConfig = fs::read_to_string(config_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(AppConfig {
                device_guid: String::new(),
                friendly_name: String::new(),
                recent_budgets: vec![],
                enter_scheduled_transactions: true,
                backup_on_first_save: true,
                csv_mappings: HashMap::new(),
                pin_hash: None,
            });
        let mut dirty = false;
        if c.device_guid.is_empty() {
            c.device_guid = budget4_core::package::new_guid();
            dirty = true;
        }
        if c.friendly_name.trim().is_empty() {
            c.friendly_name = hostname();
            dirty = true;
        }
        if dirty {
            c.save();
        }
        c
    }

    pub fn save(&self) {
        let _ = fs::create_dir_all(config_dir());
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = fs::write(config_file(), text);
        }
    }

    pub fn remember(&mut self, folder: &str) {
        self.recent_budgets.retain(|f| f != folder);
        self.recent_budgets.insert(0, folder.to_string());
        self.recent_budgets.truncate(10);
        self.save();
    }
}
