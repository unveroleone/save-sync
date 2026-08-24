use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use log::error;

use crate::{
    api::{Api, CloudManifest},
    config::Config,
    constant::{CANCEL_HINT, GAME_CARD_SAVE_DIR, GAME_SAVE_DIR},
    emulator::{emulator_kind_from_entry_id, scan_emulator_entries},
    sync::{status_for, LocalManifest, SyncStatus},
    tai::{PfsMountHandshake, Titles},
    ui::{ui_dialog::UIDialog, ui_loading::Loading, ui_toast::Toast},
    utils::{
        backup_game_save, backup_save_target, get_game_local_backup_dir,
        read_content_hash_sidecar, restore_save_target, save_target_for_downloaded_archive,
        sha256_file,
    },
    vita2d::rgba,
};

/// Not yet wired into any screen (Stage 2 of the tab-unification plan). Moved
/// here as-is from UICloud so the Games tab and a unified sync screen can
/// share it later without depending on ui_cloud.
#[derive(Clone)]
pub struct SyncGameInfo {
    pub title_id: String,
    pub name: String,
    /// Title sent to the server so backups there are labelled with the game name.
    pub server_title: String,
    /// Resolved once here so every action reads the same directory.
    pub local_dir: String,
    /// Native Vita titles only: the id used to locate GAME_CARD_SAVE_DIR /
    /// GAME_SAVE_DIR, distinct from `title_id`. None for emulator and
    /// pure-cloud entries.
    pub real_id: Option<String>,
    pub status: SyncStatus,
    pub local_time: Option<String>,
    pub cloud_time: Option<String>,
    pub cloud_size: Option<u64>,
    pub version_count: u64,
    pub has_local_backup: bool,
}

pub struct SyncEngine {
    pub games: Arc<RwLock<Vec<SyncGameInfo>>>,
    pub pending: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    pub cloud_manifest: Arc<RwLock<Option<CloudManifest>>>,
    pub fetch_at: Arc<RwLock<u64>>,
    pub pfs_mount: PfsMountHandshake,
}

impl SyncEngine {
    pub fn new() -> Self {
        SyncEngine {
            games: Arc::new(RwLock::new(Vec::new())),
            pending: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            cloud_manifest: Arc::new(RwLock::new(None)),
            fetch_at: Arc::new(RwLock::new(0)),
            pfs_mount: PfsMountHandshake::new(),
        }
    }

    /// Main-thread side of the PFS mount handshake; call once per frame once
    /// this engine is wired into a screen.
    pub fn pump(&self) {
        self.pfs_mount.pump();
    }

    pub fn fetch(&self, titles: &Titles) {
        if Arc::strong_count(&self.games) > 1 {
            return; // already fetching
        }

        let config = Config::global();
        let is_configured = config.is_configured();

        let title_list: Vec<(String, String, String)> = titles
            .iter()
            .map(|t| {
                (
                    t.title_id().to_string(),
                    t.real_id().to_string(),
                    t.name().to_string(),
                )
            })
            .collect();

        // Include emulator entries (PSP, RetroArch) so they appear in the cloud list.
        let emu_entries = scan_emulator_entries();

        let games = Arc::clone(&self.games);
        let cloud_manifest = Arc::clone(&self.cloud_manifest);
        let fetch_at = Arc::clone(&self.fetch_at);

        tokio::spawn(async move {
            let manifest = if is_configured {
                match Api::get_cloud_manifest(&config) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        error!("fetch manifest failed: {}", e);
                        None
                    }
                }
            } else {
                None
            };

            // Build per-game info, dropping sync-excluded entries.
            let mut info_list = Vec::new();
            for (title_id, real_id, name) in &title_list {
                if config.is_effectively_excluded(title_id, None) {
                    continue;
                }
                let local_dir = get_game_local_backup_dir(title_id, name);
                let info = Self::build_sync_info(
                    title_id,
                    Some(real_id.as_str()),
                    name,
                    name,
                    &local_dir,
                    &manifest,
                );
                info_list.push(info);
            }

            // Emulator entries
            let mut seen_ids: std::collections::HashSet<String> =
                title_list.iter().map(|(id, _, _)| id.clone()).collect();
            for entry in &emu_entries {
                seen_ids.insert(entry.id.clone()); // even if excluded below
                if config.is_effectively_excluded(&entry.id, Some(entry.kind)) {
                    continue;
                }
                let local_dir = entry.local_backup_dir();
                let info = Self::build_sync_info(
                    &entry.id,
                    None,
                    &entry.name,
                    &entry.server_title,
                    &local_dir,
                    &manifest,
                );
                info_list.push(info);
            }

            // Add pure-cloud entries (server has them, but no local folder).
            if let Some(ref m) = manifest {
                for (id, entry) in &m.games {
                    let kind = emulator_kind_from_entry_id(id);
                    if !seen_ids.contains(id) && !config.is_effectively_excluded(id, kind) {
                        let display_name = entry
                            .title
                            .clone()
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| id.clone());
                        let local_dir = get_game_local_backup_dir(id, id);
                        let info =
                            Self::build_sync_info(id, None, &display_name, "", &local_dir, &manifest);
                        info_list.push(info);
                    }
                }
            }

            info_list.sort_by(|a, b| {
                let a_prio = status_priority(&a.status);
                let b_prio = status_priority(&b.status);
                b_prio.cmp(&a_prio).then(a.name.cmp(&b.name))
            });

            *games.write().unwrap() = info_list;
            *cloud_manifest.write().unwrap() = manifest;
            *fetch_at.write().unwrap() = crate::utils::current_time() as u64;
        });
    }

    fn build_sync_info(
        title_id: &str,
        real_id: Option<&str>,
        name: &str,
        server_title: &str,
        local_dir: &str,
        manifest: &Option<CloudManifest>,
    ) -> SyncGameInfo {
        let (has_local, local_time, local_content) = Self::scan_local_backup(local_dir);
        let cloud_data = manifest.as_ref().and_then(|m| m.games.get(title_id));

        let last_synced_hash = LocalManifest::load()
            .games
            .get(title_id)
            .and_then(|e| e.last_synced_hash.clone());
        let status = status_for(
            has_local,
            cloud_data.is_some(),
            local_content.as_deref(),
            cloud_data.and_then(|ce| ce.content_hash.as_deref()),
            last_synced_hash.as_deref(),
        );

        SyncGameInfo {
            title_id: title_id.to_string(),
            name: name.to_string(),
            server_title: server_title.to_string(),
            local_dir: local_dir.to_string(),
            real_id: real_id.map(str::to_string),
            status,
            local_time,
            cloud_time: cloud_data.map(|c| c.latest_version.clone()),
            cloud_size: cloud_data.map(|c| c.size),
            version_count: cloud_data.map(|c| c.version_count).unwrap_or(0),
            has_local_backup: has_local,
        }
    }

    /// Newest backup zip plus its content hash: (exists, newest mtime, newest
    /// sidecar content hash). Zips from builds before sidecars return no hash.
    fn scan_local_backup(local_dir: &str) -> (bool, Option<String>, Option<String>) {
        let path = Path::new(local_dir);
        if !path.exists() {
            return (false, None, None);
        }
        let mut latest: Option<(u64, String)> = None;
        if let Ok(entries) = path.read_dir() {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if !name.ends_with(".zip") || name.ends_with(" auto.zip") {
                    continue;
                }
                if let Ok(meta) = entry.metadata() {
                    if let Ok(mod_time) = meta.modified() {
                        if let Ok(dur) = mod_time.duration_since(std::time::UNIX_EPOCH) {
                            let secs = dur.as_secs();
                            if latest.as_ref().map(|(l, _)| secs > *l).unwrap_or(true) {
                                latest = Some((secs, name));
                            }
                        }
                    }
                }
            }
        }
        let has = latest.is_some();
        let ts = latest.as_ref().map(|(s, _)| format!("{}", s));
        let content = latest
            .and_then(|(_, name)| read_content_hash_sidecar(&format!("{}/{}", local_dir, name)));
        (has, ts, content)
    }

    /// Upload a single game: find newest local zip, SHA256, POST to server.
    pub fn upload_single(&self, game: &SyncGameInfo) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }
        let zip_path = match Self::find_newest_zip(&game.local_dir) {
            Some(p) => p,
            None => {
                Toast::show("No local backup to upload.".to_string());
                return;
            }
        };
        let hash = match sha256_file(&zip_path) {
            Ok(h) => h,
            Err(_) => {
                Toast::show("Failed to hash local backup.".to_string());
                return;
            }
        };
        let ts = crate::ime::get_current_format_time().to_string();
        let tid = game.title_id.to_string();
        let n = game.name.to_string();
        let server_title = game.server_title.to_string();
        let content_hash = read_content_hash_sidecar(&zip_path).unwrap_or_default();
        let pending = Arc::clone(&self.pending);
        let games = Arc::clone(&self.games);
        let cloud_manifest = Arc::clone(&self.cloud_manifest);

        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let config = Config::global();
            match Api::upload_save(&config, &tid, &server_title, &content_hash, &zip_path, &hash, &ts) {
                Ok(_) => {
                    LocalManifest::record(&tid, &content_hash);
                    Toast::show(format!("{} uploaded.", n));
                }
                Err(e) => Toast::show(format!("Upload failed: {}", e)),
            }
            if let Ok(m) = Api::get_cloud_manifest(&config) {
                *cloud_manifest.write().unwrap() = Some(m);
            }
            games.write().unwrap().clear();
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Download a single game from server to local backup dir. PSP and
    /// RetroArch saves are also restored through the normal restore path so
    /// the live save is auto-backed up first; native saves stay download-only
    /// because restoring them needs a PFS mount from the Games tab.
    pub fn download_single(&self, game: &SyncGameInfo) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }
        let local_dir = game.local_dir.to_string();
        let _ = std::fs::create_dir_all(&local_dir);
        let dl_path = format!("{}/{}.zip", local_dir, crate::ime::get_current_format_time());
        let tid = game.title_id.to_string();
        let n = game.name.to_string();
        let pending = Arc::clone(&self.pending);
        let games = Arc::clone(&self.games);
        let cloud_manifest = Arc::clone(&self.cloud_manifest);

        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let config = Config::global();
            let mut downloaded = false;
            match Api::download_save(&config, &tid, &dl_path) {
                Ok(_) => {
                    downloaded = true;
                    if let Some(target) = save_target_for_downloaded_archive(&tid, &dl_path) {
                        match restore_save_target(&target, &dl_path) {
                            Ok(_) => Toast::show(format!("{} downloaded & restored.", n)),
                            Err(e) => {
                                error!("restore {} failed: {}", tid, e);
                                Toast::show(format!("{} downloaded; restore failed: {}", n, e));
                            }
                        }
                    } else {
                        Toast::show(format!("{} downloaded.", n));
                    }
                }
                Err(e) => Toast::show(format!("Download failed: {}", e)),
            }
            if downloaded {
                if let Ok(m) = Api::get_cloud_manifest(&config) {
                    // Stamp the downloaded zip with the server's content hash
                    // so the next scan compares this save against the cloud
                    // instead of falling back to "exists".
                    if let Some(ch) = m.games.get(&tid).and_then(|ce| ce.content_hash.clone()) {
                        let _ = std::fs::write(format!("{}.chash", dl_path), &ch);
                        LocalManifest::record(&tid, &ch);
                    }
                    *cloud_manifest.write().unwrap() = Some(m);
                }
            }
            games.write().unwrap().clear();
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    pub fn per_game_action(&self, game: &SyncGameInfo) {
        match game.status {
            SyncStatus::UploadNeeded | SyncStatus::LocalOnly => {
                if game.has_local_backup {
                    self.upload_single(game);
                } else {
                    Toast::show("No local backup. Create one in Games tab.".to_string());
                }
            }
            SyncStatus::DownloadAvailable | SyncStatus::CloudOnly => {
                self.download_single(game);
            }
            SyncStatus::Conflict => {
                // Both sides changed since the last sync (or hashes are
                // unknown). Ask twice instead of silently picking a winner:
                // upload local, else download the server version.
                if UIDialog::present("Local and server both changed. Upload local over server?") {
                    self.upload_single(game);
                } else if UIDialog::present("Download server version instead?") {
                    self.download_single(game);
                }
            }
            SyncStatus::InSync => {
                Toast::show("Already in sync.".to_string());
            }
        }
    }

    pub fn sync_all(&self) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }

        let games = self.games.read().unwrap().clone();
        // LocalOnly counts as pending upload: status is derived from
        // local-backup existence, so it never reports UploadNeeded and
        // filtering on that alone left this phase unreachable.
        let upload_needed: Vec<_> = games
            .iter()
            .filter(|g| {
                config.upload_on_sync_all
                    && matches!(g.status, SyncStatus::UploadNeeded | SyncStatus::LocalOnly)
            })
            .cloned()
            .collect();
        let download_available: Vec<_> = games
            .iter()
            .filter(|g| {
                config.download_on_sync_all
                    && matches!(g.status, SyncStatus::DownloadAvailable | SyncStatus::CloudOnly)
            })
            .cloned()
            .collect();
        let conflicts: Vec<_> = games
            .iter()
            .filter(|g| g.status == SyncStatus::Conflict)
            .map(|g| (g.title_id.clone(), g.name.clone()))
            .collect();

        if !conflicts.is_empty() {
            let names: Vec<String> = conflicts.iter().map(|(_, n)| n.clone()).collect();
            Toast::show(format!("Conflicts: {}. Resolve per-game first.", names.join(", ")));
            return;
        }

        if upload_needed.is_empty() && download_available.is_empty() {
            Toast::show("Everything is in sync!".to_string());
            return;
        }

        if !UIDialog::present(&format!(
            "Sync: {} upload(s), {} download(s)?",
            upload_needed.len(),
            download_available.len()
        )) {
            return;
        }

        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = Arc::clone(&self.cancel);
        // Start from a known state so the first game that needs a fresh
        // backup always gets a fresh mount.
        self.pfs_mount.clear();
        let pfs_mount = self.pfs_mount.clone();
        Loading::show();
        let cloud_manifest = Arc::clone(&self.cloud_manifest);
        let games_arc = Arc::clone(&self.games);

        tokio::spawn(async move {
            let config = Config::global();
            let mut ok = 0;
            let mut cancelled = false;
            let mut failures: Vec<(String, String)> = Vec::new();
            let mut downloaded: Vec<(String, String)> = Vec::new();

            for (i, game) in upload_needed.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                Loading::notify_title(format!(
                    "Uploading ({}/{})    {}",
                    i + 1,
                    upload_needed.len(),
                    CANCEL_HINT
                ));
                Loading::notify_desc(game.name.to_string());
                let zip_path = match Self::ensure_backup(game, &pfs_mount, &cancel) {
                    Ok(path) => path,
                    Err(err) => {
                        if err == "cancelled" {
                            cancelled = true;
                            break;
                        }
                        failures.push((game.title_id.clone(), err));
                        continue;
                    }
                };
                let hash = match sha256_file(&zip_path) {
                    Ok(h) => h,
                    Err(_) => {
                        failures.push((game.title_id.clone(), "hash failed".to_string()));
                        continue;
                    }
                };
                let ts = crate::ime::get_current_format_time();
                let content_hash = read_content_hash_sidecar(&zip_path).unwrap_or_default();
                match Api::upload_save(
                    &config,
                    &game.title_id,
                    &game.server_title,
                    &content_hash,
                    &zip_path,
                    &hash,
                    &ts,
                ) {
                    Ok(_) => {
                        LocalManifest::record(&game.title_id, &content_hash);
                        ok += 1;
                    }
                    Err(e) => {
                        error!("upload {} failed: {}", game.title_id, e);
                        failures.push((game.title_id.clone(), e));
                    }
                }
            }

            for (i, game) in download_available.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                Loading::notify_title(format!(
                    "Downloading ({}/{})    {}",
                    i + 1,
                    download_available.len(),
                    CANCEL_HINT
                ));
                Loading::notify_desc(game.name.to_string());
                let _ = std::fs::create_dir_all(&game.local_dir);
                let dl_path = format!(
                    "{}/{}.zip",
                    game.local_dir,
                    crate::ime::get_current_format_time()
                );
                match Api::download_save(&config, &game.title_id, &dl_path) {
                    Ok(_) => {
                        downloaded.push((game.title_id.clone(), dl_path.clone()));
                        ok += 1;
                        // PSP/RetroArch restore through the normal path, which
                        // auto-backs-up the live save first. Native titles stay
                        // download-only: restoring them needs a PFS mount from
                        // the Games tab (out of scope here, see plan doc).
                        if let Some(target) =
                            save_target_for_downloaded_archive(&game.title_id, &dl_path)
                        {
                            if let Err(e) = restore_save_target(&target, &dl_path) {
                                error!("restore {} failed: {}", game.title_id, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("download {} failed: {}", game.title_id, e);
                        failures.push((game.title_id.clone(), e));
                    }
                }
            }

            match Api::get_cloud_manifest(&config) {
                Ok(m) => {
                    for (title_id, dl_path) in &downloaded {
                        if let Some(ch) = m.games.get(title_id).and_then(|ce| ce.content_hash.clone()) {
                            let _ = std::fs::write(format!("{}.chash", dl_path), &ch);
                            LocalManifest::record(title_id, &ch);
                        }
                    }
                    *cloud_manifest.write().unwrap() = Some(m);
                }
                Err(e) => error!("re-fetch manifest failed: {}", e),
            }

            let fail_count = failures.len();
            let head = if cancelled { "Sync stopped" } else { "Sync done" };
            let msg = if fail_count == 0 {
                format!("{}: {} ok", head, ok)
            } else if fail_count <= 2 {
                let names: Vec<String> = failures
                    .iter()
                    .map(|(id, err)| format!("{}: {}", id, err))
                    .collect();
                format!("{}: {} ok, {} failed ({})", head, ok, fail_count, names.join(", "))
            } else {
                format!("{}: {} ok, {} failed", head, ok, fail_count)
            };
            Toast::show(msg);
            Loading::hide();
            pending.store(false, Ordering::Relaxed);

            if let Ok(mut games) = games_arc.write() {
                games.clear();
            }
        });
    }

    /// Newest local zip for `game`, creating one first if none exists yet.
    /// Mirrors the old game-menu "Backup All to Server", which never skipped
    /// a title just because it had never been backed up before.
    fn ensure_backup(
        game: &SyncGameInfo,
        pfs_mount: &PfsMountHandshake,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        if let Some(path) = Self::find_newest_zip(&game.local_dir) {
            return Ok(path);
        }
        let backup_to_path = format!(
            "{}/{}.zip",
            game.local_dir,
            crate::ime::get_current_format_time()
        );
        if let Some(real_id) = &game.real_id {
            let dirs = [
                format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
                format!("{}/{}", GAME_SAVE_DIR, real_id),
            ];
            let game_save_dir = dirs
                .iter()
                .find(|dir| Path::new(dir).exists())
                .ok_or_else(|| "no save data".to_string())?;
            if !pfs_mount.wait_for_mount(cancel, game_save_dir) {
                return Err("cancelled".to_string());
            }
            backup_game_save(game_save_dir, &backup_to_path).map_err(|e| format!("{:?}", e))?;
            return Ok(backup_to_path);
        }
        if let Some(entry) = scan_emulator_entries()
            .into_iter()
            .find(|e| e.id == game.title_id)
        {
            let exclusions = Config::global().psp_exclusions_for(&entry.id);
            backup_save_target(&entry.save_target_excluding(&exclusions), &backup_to_path)
                .map_err(|e| format!("{:?}", e))?;
            return Ok(backup_to_path);
        }
        Err("no local backup".to_string())
    }

    fn find_newest_zip(dir: &str) -> Option<String> {
        let path = Path::new(dir);
        if !path.exists() {
            return None;
        }
        let mut newest: Option<(String, std::time::SystemTime)> = None;
        if let Ok(entries) = path.read_dir() {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                if fname.ends_with(".zip") && !fname.ends_with(" auto.zip") {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if newest.as_ref().map(|(_, t)| mtime > *t).unwrap_or(true) {
                                newest = Some((entry.path().to_string_lossy().to_string(), mtime));
                            }
                        }
                    }
                }
            }
        }
        newest.map(|(p, _)| p)
    }

    fn status_rgb(status: &SyncStatus) -> (i32, i32, i32) {
        match status {
            SyncStatus::InSync => (0x44, 0xcc, 0x44),
            SyncStatus::UploadNeeded | SyncStatus::LocalOnly => (0x44, 0x88, 0xff),
            SyncStatus::DownloadAvailable | SyncStatus::CloudOnly => (0xff, 0xaa, 0x44),
            SyncStatus::Conflict => (0xff, 0x44, 0x44),
        }
    }

    pub fn status_color(status: &SyncStatus) -> u32 {
        let (r, g, b) = Self::status_rgb(status);
        rgba(r, g, b, 0xff)
    }

    /// Same palette, custom alpha — for overlays drawn on top of other
    /// content (e.g. a badge sitting over a game icon).
    pub fn status_color_alpha(status: &SyncStatus, a: i32) -> u32 {
        let (r, g, b) = Self::status_rgb(status);
        rgba(r, g, b, a)
    }

    pub fn status_label(status: &SyncStatus, version_count: u64) -> String {
        let base = match status {
            SyncStatus::InSync => "Synced",
            SyncStatus::UploadNeeded => "Upload",
            SyncStatus::DownloadAvailable => "Download",
            SyncStatus::Conflict => "Conflict",
            SyncStatus::LocalOnly => "Not Uploaded",
            SyncStatus::CloudOnly => "Cloud Only",
        };
        if version_count > 0 {
            format!("{} ({})", base, version_count)
        } else {
            base.to_string()
        }
    }
}

pub fn status_priority(status: &SyncStatus) -> i32 {
    match status {
        SyncStatus::Conflict => 4,
        SyncStatus::UploadNeeded => 3,
        SyncStatus::LocalOnly => 2,
        SyncStatus::DownloadAvailable => 1,
        SyncStatus::CloudOnly => 1,
        SyncStatus::InSync => 0,
    }
}
