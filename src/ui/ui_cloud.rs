pub mod list_state;

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
    app::AppData,
    config::Config,
    constant::{CANCEL_HINT, SCREEN_HEIGHT, SCREEN_WIDTH},
    sync::SyncStatus,
    ui::{ui_dialog::UIDialog, ui_loading::Loading, ui_settings::UISettings, ui_toast::Toast},
    utils::{get_active_color, get_game_local_backup_dir, restore_save_target, sha256_file},
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_text_width, SceCtrlButtons,
    },
};

use super::ui_base::UIBase;

#[derive(Clone)]
struct SyncGameInfo {
    title_id: String,
    name: String,
    /// Title sent to the server so backups there are labelled with the game name.
    server_title: String,
    /// Resolved once here so every action reads the same directory. Deriving it
    /// per call site used to make uploads and status checks disagree.
    local_dir: String,
    status: SyncStatus,
    local_time: Option<String>,
    cloud_time: Option<String>,
    cloud_size: Option<u64>,
    version_count: u64,
    has_local_backup: bool,
}

pub struct UICloud {
    games: Arc<RwLock<Vec<SyncGameInfo>>>,
    selected_idx: i32,
    top_row: i32,
    pending: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    cloud_manifest: Arc<RwLock<Option<CloudManifest>>>,
    fetch_at: Arc<RwLock<u64>>,
    show_settings: bool,
    settings: Option<UISettings>,
}

const DISPLAY_ROWS: i32 = 12;

impl UICloud {
    pub fn new() -> UICloud {
        UICloud {
            games: Arc::new(RwLock::new(Vec::new())),
            selected_idx: 0,
            top_row: 0,
            pending: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            cloud_manifest: Arc::new(RwLock::new(None)),
            fetch_at: Arc::new(RwLock::new(0)),
            show_settings: false,
            settings: None,
        }
    }

    fn fetch_sync_data(&self, titles: &crate::tai::Titles) {
        if Arc::strong_count(&self.games) > 1 {
            return; // already fetching
        }

        let config = Config::global();
        let is_configured = config.is_configured();

        let title_list: Vec<(String, String)> = titles
            .iter()
            .map(|t| (t.title_id().to_string(), t.name().to_string()))
            .collect();

        // Include emulator entries (PSP, RetroArch) so they appear in the cloud list.
        let emu_entries = crate::emulator::scan_emulator_entries();

        let games = Arc::clone(&self.games);
        let cloud_manifest = Arc::clone(&self.cloud_manifest);
        let fetch_at = Arc::clone(&self.fetch_at);

        tokio::spawn(async move {
            // Fetch cloud manifest if configured
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

            // Build per-game info
            let mut info_list = Vec::new();
            for (title_id, name) in &title_list {
                let local_dir = get_game_local_backup_dir(title_id, name);
                let info = Self::build_sync_info(
                    title_id, name, name, &local_dir, &manifest,
                );
                info_list.push(info);
            }

            // Emulator entries
            let mut seen_ids: std::collections::HashSet<String> = title_list
                .iter()
                .map(|(id, _)| id.clone())
                .collect();
            for entry in &emu_entries {
                let local_dir = entry.local_backup_dir();
                let info = Self::build_sync_info(
                    &entry.id, &entry.name, &entry.server_title, &local_dir, &manifest,
                );
                seen_ids.insert(entry.id.clone());
                info_list.push(info);
            }

            // Add pure-cloud entries (server has them, but no local folder)
            if let Some(ref m) = manifest {
                for (id, entry) in &m.games {
                    if !seen_ids.contains(id) {
                        // The server labels the save with the game title when
                        // it was uploaded with one; fall back to the id.
                        let display_name = entry
                            .title
                            .clone()
                            .filter(|t| !t.is_empty())
                            .unwrap_or_else(|| id.clone());
                        let local_dir = get_game_local_backup_dir(id, id);
                        let info = Self::build_sync_info(
                            id, &display_name, "", &local_dir, &manifest,
                        );
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
        name: &str,
        server_title: &str,
        local_dir: &str,
        manifest: &Option<CloudManifest>,
    ) -> SyncGameInfo {
        let (has_local, local_time, local_content) = Self::scan_local_backup(local_dir);
        let cloud_data = manifest
            .as_ref()
            .and_then(|m| m.games.get(title_id));

        // Status from the canonical content hashes when both sides have one;
        // otherwise the exists-based fallback from before hashes existed.
        let sync_entry = crate::sync::LocalManifest::load()
            .games
            .get(title_id)
            .cloned();
        let status = match (&has_local, cloud_data) {
            (false, None) => SyncStatus::LocalOnly,
            (false, Some(_)) => SyncStatus::CloudOnly,
            (true, None) => SyncStatus::LocalOnly,
            (true, Some(ce)) => match (&local_content, &ce.content_hash) {
                (Some(local), Some(cloud)) => {
                    if local == cloud {
                        SyncStatus::InSync
                    } else {
                        let last = sync_entry.and_then(|e| e.last_synced_hash);
                        match last {
                            // Server unchanged, local differs: push.
                            Some(last) if last == *cloud => SyncStatus::UploadNeeded,
                            // Local unchanged, server differs: pull.
                            Some(last) if last == *local => SyncStatus::DownloadAvailable,
                            // Both moved (or unknown): let the user decide.
                            _ => SyncStatus::Conflict,
                        }
                    }
                }
                // Older backups carry no content hash: trust existence.
                _ => SyncStatus::InSync,
            },
        };

        SyncGameInfo {
            title_id: title_id.to_string(),
            name: name.to_string(),
            server_title: server_title.to_string(),
            local_dir: local_dir.to_string(),
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
                // Auto-backups hold the pre-restore save and must never be
                // picked as the newest local state.
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
        let content = latest.and_then(|(_, name)| {
            crate::utils::read_content_hash_sidecar(&format!("{}/{}", local_dir, name))
        });
        (has, ts, content)
    }

    /// Upload a single game: find newest local zip, SHA256, POST to server.
    fn upload_single(&self, game: &SyncGameInfo) {
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
        let content_hash = crate::utils::read_content_hash_sidecar(&zip_path).unwrap_or_default();
        let pending = Arc::clone(&self.pending);
        let games = Arc::clone(&self.games);
        let cloud_manifest = Arc::clone(&self.cloud_manifest);

        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let config = Config::global();
            match Api::upload_save(
                &config,
                &tid,
                &server_title,
                &content_hash,
                &zip_path,
                &hash,
                &ts,
            ) {
                Ok(_) => {
                    crate::sync::LocalManifest::record(&tid, &content_hash);
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
    fn download_single(&self, game: &SyncGameInfo) {
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
                    if let Some(target) =
                        crate::utils::save_target_for_downloaded_archive(&tid, &dl_path)
                    {
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
                        crate::sync::LocalManifest::record(&tid, &ch);
                    }
                    *cloud_manifest.write().unwrap() = Some(m);
                }
            }
            games.write().unwrap().clear();
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    fn per_game_action(&self, game: &SyncGameInfo) {
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

    fn sync_all(&self) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }

        let games = self.games.read().unwrap().clone();
        // LocalOnly counts as pending upload: the cloud tab derives status from
        // local-backup existence, so it never reports UploadNeeded and filtering
        // on that alone left this phase unreachable.
        let upload_needed: Vec<_> = games
            .iter()
            .filter(|g| {
                config.upload_on_sync_all
                    && matches!(g.status, SyncStatus::UploadNeeded | SyncStatus::LocalOnly)
                    && g.has_local_backup
            })
            .cloned()
            .collect();
        let download_available: Vec<_> = games
            .iter()
            .filter(|g| {
                config.download_on_sync_all
                    && matches!(
                        g.status,
                        SyncStatus::DownloadAvailable | SyncStatus::CloudOnly
                    )
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
        Loading::show();
        let cloud_manifest = Arc::clone(&self.cloud_manifest);
        let games_arc = Arc::clone(&self.games);

        tokio::spawn(async move {
            let config = Config::global();
            let mut ok = 0;
            let mut cancelled = false;
            let mut failures: Vec<(String, String)> = Vec::new();
            let mut downloaded: Vec<(String, String)> = Vec::new();

            // Upload phase
            for (i, game) in upload_needed.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                // Both title and desc: the loading dialog stays hidden without
                // a desc, leaving only a bare spinner on screen.
                Loading::notify_title(format!(
                    "Uploading ({}/{})    {}",
                    i + 1,
                    upload_needed.len(),
                    CANCEL_HINT
                ));
                Loading::notify_desc(game.name.to_string());
                let zip_path = match Self::find_newest_zip(&game.local_dir) {
                    Some(path) => path,
                    None => {
                        failures.push((game.title_id.clone(), "no local backup".to_string()));
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
                let content_hash =
                    crate::utils::read_content_hash_sidecar(&zip_path).unwrap_or_default();
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
                        crate::sync::LocalManifest::record(&game.title_id, &content_hash);
                        ok += 1;
                    }
                    Err(e) => {
                        error!("upload {} failed: {}", game.title_id, e);
                        failures.push((game.title_id.clone(), e));
                    }
                }
            }

            // Download phase
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
                // Deliberately no extract here. Sync All is a bulk action, so it
                // only fetches archives into the backup dir; writing them into
                // SAVEDATA would overwrite live saves with no confirmation and
                // no auto-backup. Restoring stays an explicit per-game choice.
                match Api::download_save(&config, &game.title_id, &dl_path) {
                    Ok(_) => {
                        downloaded.push((game.title_id.clone(), dl_path.clone()));
                        ok += 1;
                    }
                    Err(e) => {
                        error!("download {} failed: {}", game.title_id, e);
                        failures.push((game.title_id.clone(), e));
                    }
                }
            }

            // Re-fetch manifest to update status, and stamp downloaded zips
            // with the server's content hash so the next scan compares this
            // save against the cloud instead of falling back to "exists".
            match Api::get_cloud_manifest(&config) {
                Ok(m) => {
                    for (title_id, dl_path) in &downloaded {
                        if let Some(ch) =
                            m.games.get(title_id).and_then(|ce| ce.content_hash.clone())
                        {
                            let _ = std::fs::write(format!("{}.chash", dl_path), &ch);
                            crate::sync::LocalManifest::record(title_id, &ch);
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
                format!(
                    "{}: {} ok, {} failed ({})",
                    head,
                    ok,
                    fail_count,
                    names.join(", ")
                )
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

    fn find_newest_zip(dir: &str) -> Option<String> {
        let path = Path::new(dir);
        if !path.exists() {
            return None;
        }
        let mut newest: Option<(String, std::time::SystemTime)> = None;
        if let Ok(entries) = path.read_dir() {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                // Auto-backups hold the pre-restore save; they are neither
                // the newest state nor upload candidates.
                if fname.ends_with(".zip") && !fname.ends_with(" auto.zip") {
                    if let Ok(meta) = entry.metadata() {
                        if let Ok(mtime) = meta.modified() {
                            if newest
                                .as_ref()
                                .map(|(_, t)| mtime > *t)
                                .unwrap_or(true)
                            {
                                newest = Some((entry.path().to_string_lossy().to_string(), mtime));
                            }
                        }
                    }
                }
            }
        }
        newest.map(|(p, _)| p)
    }

    fn status_color(status: &SyncStatus) -> u32 {
        match status {
            SyncStatus::InSync => rgba(0x44, 0xcc, 0x44, 0xff),
            SyncStatus::UploadNeeded | SyncStatus::LocalOnly => rgba(0x44, 0x88, 0xff, 0xff),
            SyncStatus::DownloadAvailable | SyncStatus::CloudOnly => rgba(0xff, 0xaa, 0x44, 0xff),
            SyncStatus::Conflict => rgba(0xff, 0x44, 0x44, 0xff),
        }
    }

    fn status_label(status: &SyncStatus, version_count: u64) -> String {
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

    fn scroll_list(&mut self, buttons: u32) {
        if is_button(buttons, SceCtrlButtons::SceCtrlUp) {
            self.selected_idx = (self.selected_idx - 1).max(0);
        } else if is_button(buttons, SceCtrlButtons::SceCtrlDown) {
            let size = self.games.read().unwrap().len() as i32;
            if size > 0 {
                self.selected_idx = (self.selected_idx + 1).min(size - 1);
            }
        }
        if self.selected_idx < self.top_row {
            self.top_row = self.selected_idx;
        } else if self.selected_idx - self.top_row >= DISPLAY_ROWS {
            self.top_row = self.selected_idx - DISPLAY_ROWS + 1;
        }
    }
}

fn status_priority(status: &SyncStatus) -> i32 {
    match status {
        SyncStatus::Conflict => 4,
        SyncStatus::UploadNeeded => 3,
        SyncStatus::LocalOnly => 2,
        SyncStatus::DownloadAvailable => 1,
        SyncStatus::CloudOnly => 1,
        SyncStatus::InSync => 0,
    }
}

impl UICloud {
    pub fn invalidate(&mut self) {
        self.games.write().unwrap().clear();
    }
}

impl UIBase for UICloud {
    fn invalidate(&mut self) {
        self.games.write().unwrap().clear();
    }
    fn update(&mut self, app_data: &mut AppData, buttons: u32) {
        // Handle settings mode
        if self.show_settings {
            if let Some(ref mut settings) = self.settings {
                settings.update(app_data, buttons);
                if settings.should_close {
                    if settings.dirty {
                        let config = settings.get_config().clone();
                        config.save();
                        Config::update_global(config);
                        Toast::show("Settings saved.".to_string());
                    }
                    self.show_settings = false;
                    self.settings = None;
                    self.games.write().unwrap().clear();
                }
            }
            return;
        }

        // Settings access
        if is_button(buttons, SceCtrlButtons::SceCtrlTriangle) {
            self.show_settings = true;
            self.settings = Some(UISettings::new(&Config::global()));
            return;
        }

        if self.pending.load(Ordering::Relaxed) {
            // Sync All holds all input, so circle is free to mean "stop".
            // No confirmation dialog: it would block the main loop mid-run.
            if is_button(buttons, SceCtrlButtons::SceCtrlCircle)
                && !self.cancel.swap(true, Ordering::Relaxed)
            {
                Toast::show("Stopping after this game...".to_string());
            }
            if !Loading::is_pending() {
                self.pending.store(false, Ordering::Relaxed);
            }
            return;
        }

        // Lazy fetch
        if self.games.read().unwrap().is_empty() {
            self.fetch_sync_data(&app_data.titles);
            return;
        }

        self.scroll_list(buttons);

        // Cross = per-game action (was: sync all)
        if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
            let games = self.games.read().unwrap();
            if let Some(game) = games.get(self.selected_idx as usize) {
                let g = game.clone();
                drop(games);
                self.per_game_action(&g);
            }
        }

        // Square = Sync All
        if is_button(buttons, SceCtrlButtons::SceCtrlSquare) {
            self.sync_all();
        }

        // Select = Refresh
        if is_button(buttons, SceCtrlButtons::SceCtrlSelect) {
            self.games.write().unwrap().clear();
        }
    }

    fn draw(&self, app_data: &AppData) {
        // Settings overlay
        if self.show_settings {
            if let Some(ref settings) = self.settings {
                settings.draw(app_data);
            }
            return;
        }

        let games = self.games.read().unwrap();
        if games.is_empty() && !self.pending.load(Ordering::Relaxed) {
            let msg = "No sync data. Press △ for Settings.";
            vita2d_draw_text(
                (SCREEN_WIDTH - vita2d_text_width(1.0, msg)) / 2,
                SCREEN_HEIGHT / 2,
                rgba(0xaa, 0xaa, 0xaa, 0xff),
                1.0,
                msg,
            );
        } else {
            let size = games.len() as i32;
            for idx in 0..DISPLAY_ROWS {
                let i = self.top_row + idx;
                if i >= size {
                    break;
                }
                let game = &games[i as usize];
                let x = 12;
                let y = 100 + 32 * idx;

                // Selection highlight
                if i == self.selected_idx {
                    vita2d_draw_rect(
                        x as f32,
                        (y - 2) as f32,
                        (SCREEN_WIDTH - 24) as f32,
                        30.0,
                        get_active_color(),
                    );
                    vita2d_draw_rect(
                        (x + 2) as f32,
                        y as f32,
                        (SCREEN_WIDTH - 28) as f32,
                        26.0,
                        rgba(0x18, 0x18, 0x18, 0xff),
                    );
                }

                // Game name
                let name = format!("{}  {}", game.name, game.title_id);
                let max_name_w = SCREEN_WIDTH - 200;
                let name_w = vita2d_text_width(1.0, &name);
                let display_name = if name_w > max_name_w {
                    format!("{}...", &name[..(name.len().min((max_name_w / 8) as usize))])
                } else {
                    name
                };
                vita2d_draw_text(x + 8, y + 20, rgba(0xff, 0xff, 0xff, 0xff), 1.0, &display_name);

                // Status badge with version count
                let label = Self::status_label(&game.status, game.version_count);
                let color = Self::status_color(&game.status);
                let badge_w = vita2d_text_width(1.0, &label) + 12;
                let badge_x = SCREEN_WIDTH - badge_w - 24;
                vita2d_draw_rect(badge_x as f32, y as f32, badge_w as f32, 24.0, color);
                vita2d_draw_text(
                    badge_x + 6,
                    y + 18,
                    rgba(0xff, 0xff, 0xff, 0xff),
                    1.0,
                    &label,
                );
            }
        }
    }

    fn is_forces(&self) -> bool {
        self.show_settings || self.pending.load(Ordering::Relaxed)
    }
}
