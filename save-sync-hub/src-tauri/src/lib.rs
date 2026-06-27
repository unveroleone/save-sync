mod api;
mod backup;
mod config;
mod emulator_paths;
mod sync;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tauri::State;

use api::DeviceEntry;
use config::{load_config, save_config, SyncConfig};
use emulator_paths::{get_platform_paths, Platform};
use sync::{compute_status, GameSyncEntry, LocalManifest, SyncStatus};

struct AppState {
    #[allow(dead_code)]
    manifest: Mutex<LocalManifest>,
    config: Mutex<SyncConfig>,
}

fn manifest_path() -> PathBuf {
    let mut p = dirs_data();
    p.push("save-sync-hub");
    p.push("manifest.json");
    p
}

fn dirs_data() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
    }
    #[cfg(target_os = "linux")]
    {
        let data = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            format!("{}/.local/share", home)
        });
        PathBuf::from(data)
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(appdata)
    }
    #[cfg(target_os = "android")]
    {
        PathBuf::from("/data/data/com.unveroleone.save-sync-hub")
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct SaveEntry {
    name: String,
    title_id: String,
    source_path: String,
    /// All paths to include when zipping (may include multiple PSP folders).
    all_paths: Vec<String>,
    /// Where to extract when restoring (parent dir for multi-folder PSP saves).
    restore_dir: String,
    icon_path: Option<String>,
    /// Human-readable label for the source (e.g. "PSP (PPSSPP)", "Custom: /path").
    source_label: String,
    hash: String,
    timestamp: String,
    size: u64,
    status: String,
}

#[derive(Debug, Clone, serde::Serialize)]
struct CloudSaveEntry {
    title_id: String,
    display_name: String,
    size: u64,
    timestamp: String,
    uploaded_by: String,
    version_count: u64,
    icon_path: Option<String>,
}

// Mirrors Vita's psp_title_prefix: 4 alpha letters + 5 ascii digits = 9 chars.
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

/// Auto-correct stale reference hashes after backup/restore operations.
/// Local and cloud use different hash methods (dir vs zip), so after a
/// sync operation the reference hash for one side may be from the wrong
/// domain. On scan, if cloud hasn't changed but the local reference is
/// stale, fix it silently.
fn correct_sync_refs(entry: &mut GameSyncEntry) {
    if let (Some(ref local), Some(ref cloud)) = (&entry.local_hash, &entry.cloud_hash) {
        if let Some(ref last_cloud) = entry.last_synced_cloud_hash {
            if last_cloud == cloud {
                // Cloud unchanged — if local ref is stale (or missing), auto-fix.
                if entry.last_synced_hash.as_ref().map_or(true, |h| h != local) {
                    entry.last_synced_hash = Some(local.clone());
                }
            }
        }
    }
}

fn process_single_entry(
    entries: &mut Vec<SaveEntry>,
    manifest: &mut LocalManifest,
    cloud_games: &HashMap<String, api::CloudGameEntry>,
    name: String,
    title_id: String,
    dir_path: PathBuf,
    _save_dir: &PathBuf,
    source_label: &str,
) {
    let timestamp = get_dir_modified(&dir_path).unwrap_or_default();
    let local_hash = compute_dir_hash(&dir_path).ok();
    let cloud_entry = cloud_games.get(&title_id);

    let sync_entry = manifest.games.entry(title_id.clone()).or_insert_with(|| GameSyncEntry {
        title: name.clone(),
        local_hash: None,
        local_timestamp: None,
        cloud_hash: None,
        cloud_timestamp: None,
        last_synced_hash: None,
        last_synced_cloud_hash: None,
    });
    sync_entry.title = name.clone();
    sync_entry.local_hash = local_hash.clone();
    sync_entry.local_timestamp = Some(timestamp.clone());
    if let Some(ce) = cloud_entry {
        sync_entry.cloud_hash = Some(ce.latest_hash.clone());
        sync_entry.cloud_timestamp = Some(ce.latest_version.clone());
    }

    let size = dir_size(&dir_path).unwrap_or(0);
    correct_sync_refs(sync_entry);
    let status = match compute_status(sync_entry) {
        SyncStatus::InSync => "synced",
        SyncStatus::UploadNeeded => "upload",
        SyncStatus::DownloadAvailable => "download",
        SyncStatus::Conflict => "conflict",
        SyncStatus::LocalOnly => "local_only",
        SyncStatus::CloudOnly => "cloud_only",
    };

    let source_path = dir_path.to_string_lossy().to_string();
    let icon_path = dir_path.join("ICON0.PNG");
    let icon_path = if icon_path.exists() {
        Some(icon_path.to_string_lossy().to_string())
    } else {
        None
    };

    entries.push(SaveEntry {
        name,
        title_id,
        source_path: source_path.clone(),
        all_paths: vec![source_path.clone()],
        restore_dir: source_path,
        icon_path,
        source_label: source_label.to_string(),
        hash: local_hash.unwrap_or_default(),
        timestamp,
        size,
        status: status.to_string(),
    });
}

#[tauri::command]
fn load_config_cmd(state: State<AppState>) -> SyncConfig {
    let conf = load_config();
    if let Ok(mut c) = state.config.lock() {
        *c = conf.clone();
    }
    conf
}

#[tauri::command]
fn save_config_cmd(state: State<AppState>, config: SyncConfig) -> Result<(), String> {
    save_config(&config)?;
    if let Ok(mut c) = state.config.lock() {
        *c = config;
    }
    Ok(())
}

#[tauri::command]
fn scan_saves(
    state: State<AppState>,
    platform: String,
    custom_path: Option<String>,
    source_label: Option<String>,
) -> Result<Vec<SaveEntry>, String> {
    let label = source_label.unwrap_or_else(|| platform.clone());
    // Use custom_path as an override for any platform (e.g. PSP at a
    // non-default location). Fall back to the emulator_paths default.
    let save_dir = if let Some(ref cp) = custom_path {
        let p = PathBuf::from(cp);
        if p.exists() {
            p
        } else {
            return Err(format!("Path does not exist: {}", cp));
        }
    } else {
        let plat = match platform.as_str() {
            "psp" => Platform::Psp,
            "retroarch" => Platform::RetroArch,
            "custom" => return Err("Path required for custom platform".to_string()),
            _ => return Err("Unknown platform".to_string()),
        };
        let paths = get_platform_paths(&plat).ok_or("Platform path not found for this OS")?;
        PathBuf::from(&paths.save_dir)
    };

    if !save_dir.exists() {
        return Ok(Vec::new());
    }

    let config = state.config.lock().map_err(|e| e.to_string())?;
    let cloud_games = match fetch_cloud_manifest(&config) {
        Ok(m) => m,
        Err(_) => HashMap::new(),
    };

    let mut manifest = LocalManifest::load(&manifest_path());
    let mut entries = Vec::new();

    if platform == "psp" {
        // Group PSP save folders by 9-char title prefix (matching Vita behavior).
        let mut groups: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();

        let dir_entries = save_dir
            .read_dir()
            .map_err(|e| format!("Read dir failed: {}", e))?;
        for entry in dir_entries {
            let entry = entry.map_err(|e| format!("Dir entry failed: {}", e))?;
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(prefix) = psp_title_prefix(&name) {
                groups.entry(prefix).or_default().push((name, entry.path()));
            }
        }

        let mut game_ids: Vec<String> = groups.keys().cloned().collect();
        game_ids.sort();

        for game_id in game_ids {
            let mut folders = groups.remove(&game_id).unwrap();
            folders.sort_by(|a, b| a.0.cmp(&b.0));

            let profile_idx = folders.iter().position(|(n, _)| n.contains("PROFILE"));
            let primary_idx = profile_idx.unwrap_or(0);
            let (_primary_name, primary_path) = folders[primary_idx].clone();

            let icon_path = Path::new(&primary_path).join("ICON0.PNG");
            let icon_path = if icon_path.exists() {
                Some(icon_path.to_string_lossy().to_string())
            } else {
                folders.iter().find_map(|(_, p)| {
                    let ip = p.join("ICON0.PNG");
                    ip.exists().then(|| ip.to_string_lossy().to_string())
                })
            };

            let all_paths: Vec<String> = folders.iter().map(|(_, p)| p.to_string_lossy().to_string()).collect();
            let title_id = format!("PSP_{}", game_id);
            let display_name = if folders.len() > 1 {
                format!("{} ({} slots)", game_id, folders.len())
            } else {
                game_id.clone()
            };

            let all_path_bufs: Vec<PathBuf> = folders.iter().map(|(_, p)| p.clone()).collect();
            let timestamp = all_path_bufs.iter()
                .filter_map(|p| get_dir_modified(p))
                .max()
                .unwrap_or_default();
            let local_hash = compute_dirs_hash(&all_path_bufs).ok();
            let size = all_path_bufs.iter().filter_map(|p| dir_size(p)).sum();

            let cloud_entry = cloud_games.get(&title_id);
            let sync_entry = manifest.games.entry(title_id.clone()).or_insert_with(|| GameSyncEntry {
                title: display_name.clone(),
                local_hash: None,
                local_timestamp: None,
                cloud_hash: None,
                cloud_timestamp: None,
                last_synced_hash: None,
                last_synced_cloud_hash: None,
            });
            sync_entry.title = display_name.clone();
            sync_entry.local_hash = local_hash.clone();
            sync_entry.local_timestamp = Some(timestamp.clone());
            if let Some(ce) = cloud_entry {
                sync_entry.cloud_hash = Some(ce.latest_hash.clone());
                sync_entry.cloud_timestamp = Some(ce.latest_version.clone());
            }

            correct_sync_refs(sync_entry);
            let status = match compute_status(sync_entry) {
                SyncStatus::InSync => "synced",
                SyncStatus::UploadNeeded => "upload",
                SyncStatus::DownloadAvailable => "download",
                SyncStatus::Conflict => "conflict",
                SyncStatus::LocalOnly => "local_only",
                SyncStatus::CloudOnly => "cloud_only",
            };

            entries.push(SaveEntry {
                name: display_name,
                title_id,
                source_path: primary_path.to_string_lossy().to_string(),
                all_paths,
                restore_dir: save_dir.to_string_lossy().to_string(),
                icon_path,
                source_label: label.clone(),
                hash: local_hash.unwrap_or_default(),
                timestamp,
                size,
                status: status.to_string(),
            });
        }
    } else {
        // For retroarch and custom: auto-detect PSP saves by folder naming
        // pattern and group them, otherwise treat each folder independently.
        let dir_entries: Vec<_> = save_dir
            .read_dir()
            .map_err(|e| format!("Read dir failed: {}", e))?
            .filter_map(|e| {
                let e = e.ok()?;
                let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir { return None; }
                let name = e.file_name().to_string_lossy().to_string();
                // skip RetroArch-ineligible folders
                if platform == "retroarch" {
                    const SAVE_DIRS: &[&str] = &["saves", "savefiles", "states", "savestates"];
                    if !SAVE_DIRS.contains(&name.as_str()) { return None; }
                }
                Some((name, e.path()))
            })
            .collect();

        // Check if any entries match PSP naming — if so, group them PSP-style.
        let has_psp = dir_entries.iter().any(|(n, _)| psp_title_prefix(n).is_some());
        if has_psp {
            let mut groups: HashMap<String, Vec<(String, PathBuf)>> = HashMap::new();
            for (name, path) in dir_entries {
                if let Some(prefix) = psp_title_prefix(&name) {
                    groups.entry(prefix).or_default().push((name, path));
                } else {
                    // non-PSP folder — still emit as standalone
                    let path_clone = path.clone();
                    process_single_entry(
                        &mut entries, &mut manifest, &cloud_games,
                        name.clone(), name.clone(),
                        path_clone, &save_dir, &label,
                    );
                }
            }

            let mut game_ids: Vec<String> = groups.keys().cloned().collect();
            game_ids.sort();
            for game_id in game_ids {
                let mut folders = groups.remove(&game_id).unwrap();
                folders.sort_by(|a, b| a.0.cmp(&b.0));
                let profile_idx = folders.iter().position(|(n, _)| n.contains("PROFILE"));
                let primary_idx = profile_idx.unwrap_or(0);
                let (_primary_name, primary_path) = folders[primary_idx].clone();

                let icon_path = Path::new(&primary_path).join("ICON0.PNG");
                let icon_path = if icon_path.exists() {
                    Some(icon_path.to_string_lossy().to_string())
                } else {
                    folders.iter().find_map(|(_, p)| {
                        let ip = p.join("ICON0.PNG");
                        ip.exists().then(|| ip.to_string_lossy().to_string())
                    })
                };

                let all_paths: Vec<String> = folders.iter().map(|(_, p)| p.to_string_lossy().to_string()).collect();
                let title_id = format!("PSP_{}", game_id);
                let display_name = if folders.len() > 1 {
                    format!("{} ({} slots)", game_id, folders.len())
                } else {
                    game_id.clone()
                };

                let all_path_bufs: Vec<PathBuf> = folders.iter().map(|(_, p)| p.clone()).collect();
                let timestamp = all_path_bufs.iter()
                    .filter_map(|p| get_dir_modified(p))
                    .max()
                    .unwrap_or_default();
                let local_hash = compute_dirs_hash(&all_path_bufs).ok();
                let size = all_path_bufs.iter().filter_map(|p| dir_size(p)).sum();

                let cloud_entry = cloud_games.get(&title_id);
                let sync_entry = manifest.games.entry(title_id.clone()).or_insert_with(|| GameSyncEntry {
                    title: display_name.clone(),
                    local_hash: None,
                    local_timestamp: None,
                    cloud_hash: None,
                    cloud_timestamp: None,
                    last_synced_hash: None,
                    last_synced_cloud_hash: None,
                });
                sync_entry.title = display_name.clone();
                sync_entry.local_hash = local_hash.clone();
                sync_entry.local_timestamp = Some(timestamp.clone());
                if let Some(ce) = cloud_entry {
                    sync_entry.cloud_hash = Some(ce.latest_hash.clone());
                    sync_entry.cloud_timestamp = Some(ce.latest_version.clone());
                }

                correct_sync_refs(sync_entry);
                let status = match compute_status(sync_entry) {
                    SyncStatus::InSync => "synced",
                    SyncStatus::UploadNeeded => "upload",
                    SyncStatus::DownloadAvailable => "download",
                    SyncStatus::Conflict => "conflict",
                    SyncStatus::LocalOnly => "local_only",
                    SyncStatus::CloudOnly => "cloud_only",
                };

                entries.push(SaveEntry {
                    name: display_name,
                    title_id,
                    source_path: primary_path.to_string_lossy().to_string(),
                    all_paths,
                    restore_dir: save_dir.to_string_lossy().to_string(),
                    icon_path,
                    source_label: label.clone(),
                    hash: local_hash.unwrap_or_default(),
                    timestamp,
                    size,
                    status: status.to_string(),
                });
            }
        } else {
            for (name, dir_path) in dir_entries {
                let title_id = if platform == "retroarch" {
                    format!("RETROARCH_{}", name)
                } else {
                    name.clone()
                };
                process_single_entry(
                    &mut entries, &mut manifest, &cloud_games,
                    name, title_id,
                    dir_path, &save_dir, &label,
                );
            }
        }
    }

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    manifest.updated_at = chrono_now();
    manifest.save(&manifest_path());

    Ok(entries)
}

#[tauri::command]
fn resolve_platform_path(platform: String, custom_path: Option<String>) -> Result<String, String> {
    let plat = match platform.as_str() {
        "psp" => Platform::Psp,
        "retroarch" => Platform::RetroArch,
        "custom" => {
            if let Some(p) = custom_path {
                Platform::Custom(p)
            } else {
                return Err("Custom path required".to_string());
            }
        }
        _ => return Err("Unknown platform".to_string()),
    };
    let paths = get_platform_paths(&plat).ok_or("Platform not supported on this OS")?;
    Ok(paths.save_dir)
}

#[tauri::command]
fn backup_and_upload(
    state: State<AppState>,
    title_id: String,
    all_paths: Vec<String>,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }

    let tmp_dir = temp_dir();
    let _ = std::fs::create_dir_all(&tmp_dir);
    let zip_name = format!("{}.zip", title_id);
    let zip_path = tmp_dir.join(&zip_name);

    if all_paths.len() == 1 {
        backup::zip_dir(&all_paths[0], &zip_path.to_string_lossy())?;
    } else {
        let sources: Vec<(String, String)> = all_paths
            .iter()
            .map(|p| {
                let name = Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| p.clone());
                (name, p.clone())
            })
            .collect();
        backup::zip_dirs(&sources, &zip_path.to_string_lossy())?;
    }

    // Compute local (directory) hash before zipping — this is what scan uses.
    let all_path_bufs: Vec<PathBuf> = all_paths.iter().map(PathBuf::from).collect();
    let local_hash = compute_dirs_hash(&all_path_bufs).unwrap_or_default();
    let zip_hash = backup::sha256_file(&zip_path.to_string_lossy())?;
    let timestamp = chrono_now();

    api::upload_save(&config, &title_id, &zip_path.to_string_lossy(), &zip_hash, &timestamp)?;

    let mut manifest = LocalManifest::load(&manifest_path());
    let entry = manifest.games.entry(title_id.clone()).or_insert_with(|| GameSyncEntry {
        title: title_id.clone(),
        local_hash: None,
        local_timestamp: None,
        cloud_hash: None,
        cloud_timestamp: None,
        last_synced_hash: None,
        last_synced_cloud_hash: None,
    });
    entry.title = title_id.clone();
    entry.local_hash = Some(local_hash.clone());
    entry.last_synced_hash = Some(local_hash);
    entry.cloud_hash = Some(zip_hash.clone());
    entry.last_synced_cloud_hash = Some(zip_hash);
    entry.local_timestamp = Some(timestamp.clone());
    entry.cloud_timestamp = Some(timestamp);
    manifest.updated_at = chrono_now();
    manifest.save(&manifest_path());

    let _ = std::fs::remove_file(&zip_path);
    Ok("Upload complete".to_string())
}

#[tauri::command]
fn download_and_restore(
    state: State<AppState>,
    title_id: String,
    restore_dir: String,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }

    let tmp_dir = temp_dir();
    let _ = std::fs::create_dir_all(&tmp_dir);
    let zip_path = tmp_dir.join(format!("{}.zip", title_id));

    api::download_save(&config, &title_id, &zip_path.to_string_lossy())?;
    backup::zip_extract(&zip_path.to_string_lossy(), &restore_dir)?;

    let cloud_manifest = api::get_manifest(&config)?;
    let mut manifest = LocalManifest::load(&manifest_path());
    if let Some(ce) = cloud_manifest.games.get(&title_id) {
        if let Some(entry) = manifest.games.get_mut(&title_id) {
            entry.cloud_hash = Some(ce.latest_hash.clone());
            entry.cloud_timestamp = Some(ce.latest_version.clone());
            entry.last_synced_cloud_hash = Some(ce.latest_hash.clone());
            // Clear the local reference — it will be recomputed on next scan.
            entry.last_synced_hash = None;
        }
    }
    manifest.updated_at = chrono_now();
    manifest.save(&manifest_path());

    let _ = std::fs::remove_file(&zip_path);
    Ok("Restore complete".to_string())
}

#[tauri::command]
fn download_only(
    state: State<AppState>,
    title_id: String,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }

    let tmp_dir = temp_dir();
    let _ = std::fs::create_dir_all(&tmp_dir);
    let zip_path = tmp_dir.join(format!("{}.zip", title_id));

    api::download_save(&config, &title_id, &zip_path.to_string_lossy())?;

    Ok(zip_path.to_string_lossy().to_string())
}

#[tauri::command]
fn get_cloud_saves(
    state: State<AppState>,
    search_paths: Vec<String>,
) -> Result<Vec<CloudSaveEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }

    // Try to find local icons for PSP cloud entries by matching title IDs
    // against folders in any of the local search paths.
    let icon_map: HashMap<String, String> = find_local_icons(&search_paths);

    let manifest = api::get_manifest(&config)?;
    let mut result: Vec<CloudSaveEntry> = manifest
        .games
        .into_iter()
        .map(|(title_id, entry)| {
            let display_name = if let Some(stripped) = title_id.strip_prefix("PSP_") {
                format!("PSP: {}", stripped)
            } else if let Some(stripped) = title_id.strip_prefix("RETROARCH_") {
                format!("RetroArch: {}", stripped)
            } else {
                title_id.clone()
            };
            let icon_path = icon_map.get(&title_id).cloned();
            CloudSaveEntry {
                title_id,
                display_name,
                size: entry.size,
                timestamp: entry.latest_version,
                uploaded_by: entry.uploaded_by,
                version_count: entry.version_count,
                icon_path,
            }
        })
        .collect();
    result.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(result)
}

fn find_local_icons(search_paths: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for path_str in search_paths {
        let dir = PathBuf::from(path_str);
        let dir_entries = match dir.read_dir() {
            Ok(d) => d,
            Err(_) => continue,
        };
        for entry in dir_entries.filter_map(|e| e.ok()) {
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(prefix) = psp_title_prefix(&name) {
                let title_id = format!("PSP_{}", prefix);
                let icon = entry.path().join("ICON0.PNG");
                if icon.exists() {
                    map.entry(title_id).or_insert_with(|| icon.to_string_lossy().to_string());
                }
            }
        }
    }
    map
}

#[tauri::command]
fn delete_cloud_save(state: State<AppState>, title_id: String) -> Result<(), String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }
    api::delete_save(&config, &title_id)?;
    // Remove from local manifest too
    let mut manifest = LocalManifest::load(&manifest_path());
    manifest.games.remove(&title_id);
    manifest.updated_at = chrono_now();
    manifest.save(&manifest_path());
    Ok(())
}

#[tauri::command]
fn get_devices(state: State<AppState>) -> Result<Vec<DeviceEntry>, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }
    api::get_devices(&config)
}

fn temp_dir() -> PathBuf {
    std::env::temp_dir().join("save-sync-hub")
}

fn fetch_cloud_manifest(
    config: &SyncConfig,
) -> Result<HashMap<String, api::CloudGameEntry>, String> {
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Ok(HashMap::new());
    }
    let manifest = api::get_manifest(config)?;
    Ok(manifest.games)
}

fn compute_dirs_hash(paths: &[PathBuf]) -> Result<String, String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let mut all_files: Vec<PathBuf> = Vec::new();
    for p in paths {
        walk_dir(p, &mut all_files)?;
    }
    all_files.sort();
    for f in &all_files {
        let data = std::fs::read(f).map_err(|e| format!("Read failed: {}", e))?;
        hasher.update(&data);
    }
    let hash = hasher.finalize();
    Ok(format!(
        "sha256:{}",
        hash.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    ))
}

fn compute_dir_hash(path: &PathBuf) -> Result<String, String> {
    compute_dirs_hash(std::slice::from_ref(path))
}

fn walk_dir(path: &PathBuf, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if path.is_file() {
        files.push(path.clone());
        return Ok(());
    }
    if !path.is_dir() {
        return Ok(());
    }
    for entry in path.read_dir().map_err(|e| format!("Read dir failed: {}", e))? {
        let entry = entry.map_err(|e| format!("Dir entry failed: {}", e))?;
        let p = entry.path();
        if p.is_dir() {
            walk_dir(&p, files)?;
        } else {
            files.push(p);
        }
    }
    Ok(())
}

fn get_dir_modified(path: &PathBuf) -> Option<String> {
    let metadata = path.metadata().ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(format!("{}", duration.as_secs()))
}

fn dir_size(path: &PathBuf) -> Option<u64> {
    let mut total = 0u64;
    let mut files: Vec<PathBuf> = Vec::new();
    walk_dir(path, &mut files).ok()?;
    for f in &files {
        if let Ok(meta) = f.metadata() {
            total += meta.len();
        }
    }
    Some(total)
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days_since_epoch = secs / 86400;
    let mut year = 1970i64;
    let mut remaining = days_since_epoch as i64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let month_days = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut month = 0;
    while month < 12 && remaining >= month_days[month] {
        remaining -= month_days[month];
        month += 1;
    }
    let day = remaining + 1;
    let time_secs = now.as_secs() % 86400;
    let hours = time_secs / 3600;
    let minutes = (time_secs % 3600) / 60;
    let seconds = time_secs % 60;
    // ISO 8601 format so new Date() can parse it.
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year,
        month + 1,
        day,
        hours,
        minutes,
        seconds
    )
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState {
        manifest: Mutex::new(LocalManifest::default()),
        config: Mutex::new(load_config()),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            load_config_cmd,
            save_config_cmd,
            scan_saves,
            backup_and_upload,
            download_and_restore,
            download_only,
            resolve_platform_path,
            get_cloud_saves,
            delete_cloud_save,
            get_devices,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
