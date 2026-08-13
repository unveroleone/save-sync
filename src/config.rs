use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;
use std::{fs, path::Path, sync::RwLock};

use crate::constant::CONFIG_PATH;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    pub api_token: String,
    pub device_name: String,
    pub verify_hashes: bool,
    pub upload_on_sync_all: bool,
    pub download_on_sync_all: bool,
    /// PSP folders excluded from backups, keyed by entry id (e.g.
    /// "PSP_ULJM05800" -> ["ULJM05800INS"]). Exclusions (not inclusions) so a
    /// new folder suffix is backed up by default.
    #[serde(default)]
    pub psp_folder_exclusions: HashMap<String, Vec<String>>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            server_url: String::new(),
            api_token: String::new(),
            device_name: "vita".to_string(),
            verify_hashes: true,
            upload_on_sync_all: true,
            download_on_sync_all: true,
            psp_folder_exclusions: HashMap::new(),
        }
    }
}

static CONFIG: OnceLock<RwLock<Config>> = OnceLock::new();

fn config_lock() -> &'static RwLock<Config> {
    CONFIG.get_or_init(|| RwLock::new(Config::load()))
}

impl Config {
    pub fn load() -> Config {
        if let Ok(data) = fs::read_to_string(CONFIG_PATH) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            Config::default()
        }
    }

    pub fn save(&self) {
        if let Some(parent) = Path::new(CONFIG_PATH).parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(CONFIG_PATH, json);
        }
    }

    pub fn is_configured(&self) -> bool {
        !self.server_url.is_empty() && !self.api_token.is_empty()
    }

    pub fn psp_exclusions_for(&self, entry_id: &str) -> Vec<String> {
        self.psp_folder_exclusions
            .get(entry_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_psp_exclusions(&mut self, entry_id: &str, exclusions: Vec<String>) {
        if exclusions.is_empty() {
            self.psp_folder_exclusions.remove(entry_id);
        } else {
            self.psp_folder_exclusions
                .insert(entry_id.to_string(), exclusions);
        }
    }

    pub fn global() -> Config {
        config_lock().read().expect("config read lock").clone()
    }

    pub fn update_global(config: Config) {
        *config_lock().write().expect("config write lock") = config;
    }
}
