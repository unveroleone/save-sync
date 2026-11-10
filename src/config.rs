use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;
use std::{fs, path::Path, sync::RwLock};

use crate::{constant::CONFIG_PATH, emulator::EmulatorKind};

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
    /// Entry ids excluded from sync entirely (dropped from the Games grid,
    /// not just unbadged).
    #[serde(default)]
    pub sync_excluded_entries: HashSet<String>,
    /// Exclude a whole platform at once (native/PSP/PSX/RetroArch).
    #[serde(default)]
    pub sync_exclude_all_native: bool,
    #[serde(default)]
    pub sync_exclude_all_psp: bool,
    #[serde(default)]
    pub sync_exclude_all_psx: bool,
    #[serde(default)]
    pub sync_exclude_all_retroarch: bool,
    /// Convert Adrenaline PSX memory cards to raw format for sync, instead
    /// of syncing the native VMP bytes as-is. Off by default — when off,
    /// PSX titles are treated exactly like regular PSP saves.
    #[serde(default)]
    pub convert_psx_saves: bool,
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
            sync_excluded_entries: HashSet::new(),
            sync_exclude_all_native: false,
            sync_exclude_all_psp: false,
            sync_exclude_all_psx: false,
            sync_exclude_all_retroarch: false,
            convert_psx_saves: false,
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

    pub fn is_sync_excluded(&self, entry_id: &str) -> bool {
        self.sync_excluded_entries.contains(entry_id)
    }

    pub fn set_sync_excluded(&mut self, entry_id: &str, excluded: bool) {
        if excluded {
            self.sync_excluded_entries.insert(entry_id.to_string());
        } else {
            self.sync_excluded_entries.remove(entry_id);
        }
    }

    /// `kind` is `None` for native Vita titles.
    pub fn category_excluded(&self, kind: Option<EmulatorKind>) -> bool {
        match kind {
            None => self.sync_exclude_all_native,
            Some(EmulatorKind::Psp) => self.sync_exclude_all_psp,
            Some(EmulatorKind::Psx) => self.sync_exclude_all_psx,
            Some(EmulatorKind::RetroArch) => self.sync_exclude_all_retroarch,
        }
    }

    /// Single source of truth for "does this entry sync at all".
    pub fn is_effectively_excluded(&self, id: &str, kind: Option<EmulatorKind>) -> bool {
        self.category_excluded(kind) || self.is_sync_excluded(id)
    }

    pub fn global() -> Config {
        config_lock().read().expect("config read lock").clone()
    }

    pub fn update_global(config: Config) {
        *config_lock().write().expect("config write lock") = config;
    }

    /// Saves and makes `config` the new global.
    pub fn commit(config: Config) {
        config.save();
        Config::update_global(config);
    }

    /// Read-modify-write the global config in one call.
    pub fn update(mutator: impl FnOnce(&mut Config)) {
        let mut config = Config::global();
        mutator(&mut config);
        Config::commit(config);
    }
}
