mod api;
mod backup;
mod config;
mod emulator_paths;
mod sync;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use tauri::State;

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
    hash: String,
    timestamp: String,
    size: u64,
    status: String,
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
) -> Result<Vec<SaveEntry>, String> {
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

    let paths = get_platform_paths(&plat).ok_or("Platform path not found for this OS")?;
    let save_dir = PathBuf::from(&paths.save_dir);
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

    let dir_entries = save_dir
        .read_dir()
        .map_err(|e| format!("Read dir failed: {}", e))?;

    for entry in dir_entries {
        let entry = entry.map_err(|e| format!("Dir entry failed: {}", e))?;
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        let title_id = match platform.as_str() {
            "psp" => format!("PSP_{}", name),
            "retroarch" => {
                const SAVE_DIRS: &[&str] = &["saves", "savefiles", "states", "savestates"];
                if !SAVE_DIRS.contains(&name.as_str()) {
                    continue;
                }
                format!("RETROARCH_{}", name)
            }
            _ => name.clone(),
        };

        let dir_path = entry.path();
        let timestamp = get_dir_modified(&dir_path).unwrap_or_default();

        let local_hash = compute_dir_hash(&dir_path).ok();
        let cloud_entry = cloud_games.get(&title_id);

        let sync_entry = manifest.games.entry(title_id.clone()).or_insert_with(
            || GameSyncEntry {
                title: name.clone(),
                local_hash: None,
                local_timestamp: None,
                cloud_hash: None,
                cloud_timestamp: None,
                last_synced_hash: None,
            },
        );

        sync_entry.title = name.clone();
        sync_entry.local_hash = local_hash.clone();
        sync_entry.local_timestamp = Some(timestamp.clone());
        if let Some(ce) = cloud_entry {
            sync_entry.cloud_hash = Some(ce.latest_hash.clone());
            sync_entry.cloud_timestamp = Some(ce.latest_version.clone());
        }

        let size = dir_size(&dir_path).unwrap_or(0);
        let status = match compute_status(sync_entry) {
            SyncStatus::InSync => "synced",
            SyncStatus::UploadNeeded => "upload",
            SyncStatus::DownloadAvailable => "download",
            SyncStatus::Conflict => "conflict",
            SyncStatus::LocalOnly => "local_only",
            SyncStatus::CloudOnly => "cloud_only",
        };

        let source_path = save_dir.join(&name).to_string_lossy().to_string();

        entries.push(SaveEntry {
            name,
            title_id,
            source_path,
            hash: local_hash.unwrap_or_default(),
            timestamp,
            size,
            status: status.to_string(),
        });
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
    source_path: String,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }

    let tmp_dir = temp_dir();
    let _ = std::fs::create_dir_all(&tmp_dir);
    let zip_name = format!("{}.zip", title_id);
    let zip_path = tmp_dir.join(&zip_name);

    backup::zip_dir(&source_path, &zip_path.to_string_lossy())?;
    let hash = backup::sha256_file(&zip_path.to_string_lossy())?;
    let timestamp = chrono_now();

    api::upload_save(&config, &title_id, &zip_path.to_string_lossy(), &hash, &timestamp)?;

    let mut manifest = LocalManifest::load(&manifest_path());
    if let Some(entry) = manifest.games.get_mut(&title_id) {
        entry.local_hash = Some(hash.clone());
        entry.last_synced_hash = Some(hash.clone());
        entry.local_timestamp = Some(timestamp.clone());
        entry.cloud_hash = Some(hash.clone());
        entry.cloud_timestamp = Some(timestamp);
    }
    manifest.updated_at = chrono_now();
    manifest.save(&manifest_path());

    let _ = std::fs::remove_file(&zip_path);
    Ok("Upload complete".to_string())
}

#[tauri::command]
fn download_and_restore(
    state: State<AppState>,
    title_id: String,
    target_path: String,
) -> Result<String, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    if config.server_url.is_empty() || config.api_token.is_empty() {
        return Err("Server not configured".to_string());
    }

    let tmp_dir = temp_dir();
    let _ = std::fs::create_dir_all(&tmp_dir);
    let zip_path = tmp_dir.join(format!("{}.zip", title_id));

    api::download_save(&config, &title_id, &zip_path.to_string_lossy())?;
    backup::zip_extract(&zip_path.to_string_lossy(), &target_path)?;

    let cloud_manifest = api::get_manifest(&config)?;
    let mut manifest = LocalManifest::load(&manifest_path());
    if let Some(ce) = cloud_manifest.games.get(&title_id) {
        if let Some(entry) = manifest.games.get_mut(&title_id) {
            entry.cloud_hash = Some(ce.latest_hash.clone());
            entry.cloud_timestamp = Some(ce.latest_version.clone());
            entry.last_synced_hash = Some(ce.latest_hash.clone());
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

    let result = zip_path.to_string_lossy().to_string();
    Ok(result)
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

fn compute_dir_hash(path: &PathBuf) -> Result<String, String> {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    let mut files: Vec<PathBuf> = Vec::new();
    walk_dir(path, &mut files)?;
    files.sort();
    for f in &files {
        let data = std::fs::read(f).map_err(|e| format!("Read failed: {}", e))?;
        hasher.update(&data);
    }
    let hash = hasher.finalize();
    Ok(format!(
        "sha256:{}",
        hash.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    ))
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
    // Simple ISO-like format: YYYY-MM-DD_HH-MM-SS
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
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
