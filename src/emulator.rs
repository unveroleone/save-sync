use std::{collections::HashMap, fs, path::Path};

use crate::{
    constant::{
        PSP_SAVE_DIR, RETROARCH_DIR, RETROARCH_SAVE_EXTS, RETROARCH_SAVE_SUBDIRS,
        RETROARCH_SCAN_DEPTH,
    },
    utils::{get_game_local_backup_dir, SaveTarget},
};

#[derive(Debug, Clone, PartialEq)]
pub enum EmulatorKind {
    Psp,
    RetroArch,
}

#[derive(Debug, Clone)]
pub struct EmulatorEntry {
    pub id: String,
    pub name: String,
    /// Title sent to the server so backups there are labelled with the game
    /// name (PARAM.SFO TITLE for PSP, empty when unknown). Empty skips the
    /// label and keeps the plain id folder.
    pub server_title: String,
    /// Primary save folder (PROFILE if available, otherwise DATA/first found).
    pub source_path: String,
    /// Additional save folders that belong to the same game (e.g. PROFILE + DATA).
    pub extra_paths: Vec<String>,
    pub kind: EmulatorKind,
    pub icon_path: Option<String>,
}

impl EmulatorEntry {
    /// Every save folder that belongs to this entry.
    pub fn all_paths(&self) -> Vec<String> {
        let mut paths = vec![self.source_path.to_string()];
        paths.extend(self.extra_paths.iter().cloned());
        paths
    }

    /// What to archive and where to restore it. PSP games own one folder per
    /// save slot, so they are archived under their folder names and restored
    /// into the shared SAVEDATA parent. RetroArch entries are named save files
    /// below the shared savefiles/savestates dirs, restored in place.
    pub fn save_target(&self) -> SaveTarget {
        self.save_target_excluding(&[])
    }

    /// Same target with the given folder names left out (backup-scope only;
    /// restores keep following whatever the archive holds). Excluded folders
    /// stay untouched by both backup and restore. Never returns an empty
    /// target: excluding every folder falls back to all of them.
    pub fn save_target_excluding(&self, exclusions: &[String]) -> SaveTarget {
        let mut paths: Vec<String> = self
            .all_paths()
            .into_iter()
            .filter(|p| {
                !exclusions.iter().any(|ex| {
                    Path::new(p)
                        .file_name()
                        .map(|n| n.to_string_lossy() == *ex)
                        .unwrap_or(false)
                })
            })
            .collect();
        if paths.is_empty() {
            paths = self.all_paths();
        }
        match self.kind {
            EmulatorKind::Psp => SaveTarget::grouped(&paths, PSP_SAVE_DIR),
            EmulatorKind::RetroArch => SaveTarget::files(&paths, RETROARCH_DIR),
        }
    }

    /// Local backup directory for this entry, resolved through the same helper
    /// every other caller uses so the Cloud tab finds what the Games tab wrote.
    pub fn local_backup_dir(&self) -> String {
        let safe_name = match self.kind {
            // PSP uses the raw id so the path also matches a cloud-only entry
            // downloaded before the game was detected on this device.
            EmulatorKind::Psp => self.id.to_string(),
            EmulatorKind::RetroArch => self.name.to_string(),
        };
        get_game_local_backup_dir(&self.id, &safe_name)
    }
}

/// PSP save folders follow <TITLEID><SUFFIX> where TITLEID is always 9 chars:
/// 4 ASCII letters + 5 ASCII digits (e.g. UCES01473, ULUS10234).
pub fn psp_title_prefix(folder: &str) -> Option<String> {
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

/// PARAM.SFO header (matches the layout in extern/VitaShell/sfo.h): magic
/// "\0PSF", version, key-table offset, data-table offset, index entry count —
/// all little-endian u32s, 20 bytes total. Each 16-byte index entry is
/// key_offset u16, param_fmt u16, param_len u32, param_max_len u32,
/// data_offset u32.
pub fn sfo_string(folder: &str, key: &str) -> Option<String> {
    let data = fs::read(format!("{}/PARAM.SFO", folder)).ok()?;
    if data.len() < 20 || data[0..4] != [0x00, 0x50, 0x53, 0x46] {
        return None;
    }
    let key_off = u32::from_le_bytes(data[8..12].try_into().ok()?) as usize;
    let data_off = u32::from_le_bytes(data[12..16].try_into().ok()?) as usize;
    let count = u32::from_le_bytes(data[16..20].try_into().ok()?) as usize;

    for i in 0..count {
        let start = key_off.checked_add(i.checked_mul(16)?)?;
        let idx = data.get(start..start.checked_add(16)?)?;
        let koff = key_off.checked_add(u16::from_le_bytes([idx[0], idx[1]]) as usize)?;
        let found = cstr_at(&data, koff)?;
        if found != key {
            continue;
        }
        let dlen = u32::from_le_bytes(idx[4..8].try_into().ok()?) as usize;
        let doff = data_off.checked_add(u32::from_le_bytes(idx[12..16].try_into().ok()?) as usize)?;
        let raw = data.get(doff..doff.checked_add(dlen)?)?;
        let value = raw.split(|b| *b == 0).next()?.to_vec();
        return String::from_utf8(value).ok().filter(|s| !s.is_empty());
    }
    None
}

fn cstr_at(data: &[u8], off: usize) -> Option<String> {
    let rest = data.get(off..)?;
    let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
    String::from_utf8(rest[..end].to_vec()).ok()
}

/// Game title from the save folder's PARAM.SFO (empty for unknown encodings).
pub fn psp_save_title(folder: &str) -> Option<String> {
    sfo_string(folder, "TITLE")
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

            // PARAM.SFO lives in the DATA slot, so look through every folder.
            let game_title = folders.iter().find_map(|(_, p)| psp_save_title(p));

            let display_name = match (&game_title, folders.len() > 1) {
                (Some(t), true) => format!("PSP: {} - {} ({} slots)", game_id, t, folders.len()),
                (Some(t), false) => format!("PSP: {} - {}", game_id, t),
                (None, true) => format!("PSP: {} ({} slots)", game_id, folders.len()),
                (None, false) => format!("PSP: {}", game_id),
            };

            entries.push(EmulatorEntry {
                id: format!("PSP_{}", game_id),
                name: display_name,
                server_title: game_title.unwrap_or_default(),
                source_path: primary_path,
                extra_paths,
                kind: EmulatorKind::Psp,
                icon_path,
            });
        }
    }

    // RetroArch — one entry per game. Each game's saves share its ROM name
    // (Mario.srm, Mario.sav, Mario.state1), so they are collected by name
    // instead of backing up the whole ux0:data/retroarch folder, which would
    // also sweep configs, cores and BIOS files into every backup.
    let mut ra_games: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for subdir in RETROARCH_SAVE_SUBDIRS {
        let root = format!("{}/{}", RETROARCH_DIR, subdir);
        // "sort saves by core" nests one extra level; the same names are
        // collected either way.
        collect_retroarch_saves(
            &root,
            &format!("{}/", subdir),
            RETROARCH_SCAN_DEPTH,
            &mut ra_games,
        );
    }

    let mut ra_names: Vec<String> = ra_games.keys().cloned().collect();
    ra_names.sort();

    for name in ra_names {
        let mut files = ra_games.remove(&name).unwrap();
        files.sort();

        // First file is the primary source so hashing and icon probing have
        // something stable to point at.
        let source_path = files
            .first()
            .map(|(_, path)| path.clone())
            .unwrap_or_else(|| format!("{}/{}", RETROARCH_DIR, name));
        let extra_paths: Vec<String> = files
            .iter()
            .skip(1)
            .map(|(_, path)| path.clone())
            .collect();

        entries.push(EmulatorEntry {
            // Same scheme as the Save Sync Hub so a save uploaded from one
            // device lines up with the same game on the other.
            id: format!("RETROARCH_{}", name),
            name: format!("RA: {}", name),
            // The id already carries the file name, so no extra server label.
            server_title: String::new(),
            source_path,
            extra_paths,
            kind: EmulatorKind::RetroArch,
            icon_path: None,
        });
    }

    entries
}

/// RetroArch save extensions: battery/memory-card saves plus savestates,
/// which are named `<rom>.state`, `<rom>.state1`..`<rom>.state9`, and
/// `<rom>.state.auto`.
fn is_retroarch_save_ext(ext: &str) -> bool {
    if RETROARCH_SAVE_EXTS.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
        return true;
    }
    if let Some(rest) = ext.strip_prefix("state") {
        return rest.is_empty()
            || rest.chars().all(|c| c.is_ascii_digit())
            || rest == ".auto";
    }
    false
}

/// Collect RetroArch save files below `dir` into `games`, keyed by ROM name.
/// `rel_root` is the entry-name prefix that places the file back into the same
/// subfolder on restore (e.g. "savefiles/"). Entries are `(zip_entry_name,
/// fs_path)`. The depth limit keeps the scan out of core-specific config
/// folders that happen to sit under the save dirs.
fn collect_retroarch_saves(
    dir: &str,
    rel_root: &str,
    depth: u32,
    games: &mut HashMap<String, Vec<(String, String)>>,
) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.filter_map(|e| e.ok()) {
        let path = entry.path();
        let is_dir = path.is_dir();
        if is_dir && depth == 0 {
            continue;
        }
        if is_dir {
            let rel = format!("{}{}/", rel_root, entry.file_name().to_string_lossy());
            collect_retroarch_saves(&path.to_string_lossy(), &rel, depth - 1, games);
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().to_string();
        let (stem, ext) = match file_name.rfind('.') {
            Some(dot) => (&file_name[..dot], &file_name[dot + 1..]),
            None => (&file_name[..], ""),
        };
        if stem.is_empty() || !is_retroarch_save_ext(ext) {
            continue;
        }
        // Keyed by ROM-name stem so a game's .srm and .state1 files stay in
        // one entry instead of splitting into one entry per file type.
        games
            .entry(stem.to_string())
            .or_default()
            .push((format!("{}{}", rel_root, file_name), path.to_string_lossy().to_string()));
    }
}
