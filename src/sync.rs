use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fs, path::Path};

use crate::constant::LOCAL_MANIFEST_PATH;

#[derive(Debug, Clone, PartialEq)]
pub enum SyncStatus {
    InSync,
    UploadNeeded,
    DownloadAvailable,
    Conflict,
    LocalOnly,
    CloudOnly,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GameSyncEntry {
    pub title: String,
    pub local_hash: Option<String>,
    pub local_timestamp: Option<String>,
    pub cloud_hash: Option<String>,
    pub cloud_timestamp: Option<String>,
    pub last_synced_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LocalManifest {
    pub updated_at: String,
    pub games: HashMap<String, GameSyncEntry>,
}

impl LocalManifest {
    pub fn load() -> LocalManifest {
        if let Ok(data) = fs::read_to_string(LOCAL_MANIFEST_PATH) {
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            LocalManifest::default()
        }
    }

    pub fn save(&self) {
        if let Some(parent) = Path::new(LOCAL_MANIFEST_PATH).parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(LOCAL_MANIFEST_PATH, json);
        }
    }
}

pub fn compute_status(entry: &GameSyncEntry) -> SyncStatus {
    match (
        &entry.local_hash,
        &entry.cloud_hash,
        &entry.last_synced_hash,
    ) {
        (None, None, _) => SyncStatus::LocalOnly,
        (Some(_), None, _) => SyncStatus::LocalOnly,
        (None, Some(_), _) => SyncStatus::CloudOnly,
        (Some(local), Some(cloud), last_synced) => {
            if local == cloud {
                return SyncStatus::InSync;
            }
            let local_changed = last_synced.as_ref().map(|h| h != local).unwrap_or(true);
            let cloud_changed = last_synced.as_ref().map(|h| h != cloud).unwrap_or(true);
            match (local_changed, cloud_changed) {
                (true, false) => SyncStatus::UploadNeeded,
                (false, true) => SyncStatus::DownloadAvailable,
                (true, true) => SyncStatus::Conflict,
                (false, false) => SyncStatus::InSync,
            }
        }
    }
}
