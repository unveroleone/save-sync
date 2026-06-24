use std::{collections::HashMap, fs, path::Path};

use crate::constant::{PSP_SAVE_DIR, RETROARCH_DIR};

#[derive(Debug, Clone, PartialEq)]
pub enum EmulatorKind {
    Psp,
    RetroArch,
}

#[derive(Debug, Clone)]
pub struct EmulatorEntry {
    pub id: String,
    pub name: String,
    /// Primary save folder (PROFILE if available, otherwise DATA/first found).
    pub source_path: String,
    /// Additional save folders that belong to the same game (e.g. PROFILE + DATA).
    pub extra_paths: Vec<String>,
    pub kind: EmulatorKind,
    pub icon_path: Option<String>,
}

/// PSP save folders follow <TITLEID><SUFFIX> where TITLEID is always 9 chars:
/// 4 ASCII letters + 5 ASCII digits (e.g. UCES01473, ULUS10234).
fn psp_title_prefix(folder: &str) -> Option<String> {
    if folder.len() < 9 {
        return None;
    }
    let prefix = &folder[..9];
    let b = prefix.as_bytes();
    if b[..4].iter().all(|c| c.is_ascii_alphabetic()) && b[4..].iter().all(|c| c.is_ascii_digit()) {
        Some(prefix.to_string())
    } else {
        None
    }
}

pub fn scan_emulator_entries() -> Vec<EmulatorEntry> {
    let mut entries = Vec::new();

    // PSP/Adrenaline saves — group folders by 9-char title ID.
    if let Ok(dir) = fs::read_dir(PSP_SAVE_DIR) {
        // title_id -> vec of (folder_name, full_path)
        let mut groups: HashMap<String, Vec<(String, String)>> = HashMap::new();

        for e in dir.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
            let folder = e.file_name().to_string_lossy().to_string();
            if let Some(prefix) = psp_title_prefix(&folder) {
                let full_path = format!("{}/{}", PSP_SAVE_DIR, folder);
                groups.entry(prefix).or_default().push((folder, full_path));
            }
        }

        let mut game_ids: Vec<String> = groups.keys().cloned().collect();
        game_ids.sort();

        for game_id in game_ids {
            let mut folders = groups.remove(&game_id).unwrap();
            folders.sort_by(|a, b| a.0.cmp(&b.0));

            // Prefer PROFILE folder for the icon; fall back to first DATA folder.
            let profile_idx = folders.iter().position(|(name, _)| name.contains("PROFILE"));
            let primary_idx = profile_idx.unwrap_or(0);
            let (primary_name, primary_path) = folders[primary_idx].clone();

            let icon_path = Path::new(&primary_path).join("ICON0.PNG");
            let icon_path = if icon_path.exists() {
                Some(icon_path.to_string_lossy().to_string())
            } else {
                // try other folders
                folders.iter().find_map(|(_, p)| {
                    let ip = Path::new(p).join("ICON0.PNG");
                    ip.exists().then(|| ip.to_string_lossy().to_string())
                })
            };

            let extra_paths: Vec<String> = folders
                .iter()
                .filter(|(n, _)| n != &primary_name)
                .map(|(_, p)| p.clone())
                .collect();

            let display_name = if folders.len() > 1 {
                format!("PSP: {} ({} slots)", game_id, folders.len())
            } else {
                format!("PSP: {}", game_id)
            };

            entries.push(EmulatorEntry {
                id: format!("PSP_{}", game_id),
                name: display_name,
                source_path: primary_path,
                extra_paths,
                kind: EmulatorKind::Psp,
                icon_path,
            });
        }
    }

    // RetroArch — single entry for all saves
    if Path::new(RETROARCH_DIR).exists() {
        entries.push(EmulatorEntry {
            id: "RETROARCH".to_string(),
            name: "RetroArch".to_string(),
            source_path: RETROARCH_DIR.to_string(),
            extra_paths: Vec::new(),
            kind: EmulatorKind::RetroArch,
            icon_path: None,
        });
    }

    entries
}
