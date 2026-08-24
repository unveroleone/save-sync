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
    emulator::{emulator_kind_from_entry_id, scan_emulator_entries, EmulatorEntry},
    sync::{status_for, LocalManifest, SyncStatus},
    tai::{PfsMountHandshake, Titles},
    ui::{ui_dialog::UIDialog, ui_loading::Loading, ui_toast::Toast},
    utils::{
        backup_game_save, backup_save_target, content_hash_sources, get_game_local_backup_dir,
        read_content_hash_sidecar, restore_save_target, save_target_for_downloaded_archive,
        sha256_file, SaveTarget,
    },
    vita2d::rgba,
};

/// Shared wording for every call site that refuses to act because sync
/// status isn't confirmed yet — the situation is the same (nothing
/// trustworthy to act on right now), but the reason differs, so callers
/// pass it in rather than getting a one-size-fits-all message.
fn status_not_ready(reason: &str) -> String {
    format!("Sync status not ready ({}), try again in a moment.", reason)
}

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
    /// `status` is still the provisional (last-backup based) guess — the
    /// live check (a directory hash for emulator entries, a PFS mount for
    /// native titles) hasn't reached this entry yet. Acting on `status`
    /// while this is true risks syncing against stale data, so actions
    /// must refuse until it clears. Always false for pure-cloud entries,
    /// which have no local data to verify.
    pub checking: bool,
}

/// A local entry still queued for live-status verification in `fetch()`.
/// Emulator entries just need a directory hash; native titles need a PFS
/// mount first, so they're processed after all emulator entries.
enum CheckTarget {
    Emulator(EmulatorEntry),
    Native { title_id: String, real_id: String },
}

pub struct SyncEngine {
    pub games: Arc<RwLock<Vec<SyncGameInfo>>>,
    pub pending: Arc<AtomicBool>,
    pub cancel: Arc<AtomicBool>,
    pub cloud_manifest: Arc<RwLock<Option<CloudManifest>>>,
    pub fetch_at: Arc<RwLock<u64>>,
    pub pfs_mount: PfsMountHandshake,
    /// False until a fetch has actually succeeded, and again whenever one
    /// fails. `games` is only trustworthy while this is true — badges and
    /// sync actions must not act on data we couldn't confirm is current.
    pub data_valid: Arc<AtomicBool>,
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
            data_valid: Arc::new(AtomicBool::new(false)),
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
        // Throttle retries after a failed attempt so a dead connection
        // doesn't get hammered once per frame while `games` stays empty.
        let last_attempt = *self.fetch_at.read().unwrap();
        if last_attempt > 0 && crate::utils::current_time() as u64 - last_attempt < 5 {
            return;
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
        let data_valid = Arc::clone(&self.data_valid);
        let pending = Arc::clone(&self.pending);
        let cancel = Arc::clone(&self.cancel);
        let pfs_mount = self.pfs_mount.clone();

        tokio::spawn(async move {
            let manifest = if is_configured {
                match Api::get_cloud_manifest(&config) {
                    Ok(m) => Some(m),
                    Err(e) => {
                        // A fetch failure (no network, server down, ...) is
                        // not the same as the server having no data. Acting
                        // on stale or fabricated status here risks a wrong
                        // upload/download, so clear everything instead:
                        // badges disappear and sync actions refuse to run
                        // until a fetch actually succeeds again.
                        error!("fetch manifest failed: {}", e);
                        // Only announce the transition into "can't reach the
                        // server", not every throttled retry while it stays
                        // down — otherwise this toast never stops popping up.
                        if data_valid.swap(false, Ordering::Relaxed) {
                            Toast::show(format!("Couldn't reach server: {}", e));
                        }
                        games.write().unwrap().clear();
                        *fetch_at.write().unwrap() = crate::utils::current_time() as u64;
                        return;
                    }
                }
            } else {
                None
            };

            // Fast skeleton pass: every entry's status starts from cached,
            // mount-free data (the last local backup's hash) so the grid
            // paints immediately. Anything with live save data to verify is
            // marked `checking` and queued below — emulator entries first
            // (a plain directory hash, no mount) and native titles after
            // (behind a PFS mount), so a long queue of native titles never
            // holds up the much faster emulator ones.
            let mut info_list = Vec::new();
            let mut native_checks: Vec<CheckTarget> = Vec::new();
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
                    None,
                    true,
                );
                native_checks.push(CheckTarget::Native {
                    title_id: title_id.clone(),
                    real_id: real_id.clone(),
                });
                info_list.push(info);
            }

            let mut seen_ids: std::collections::HashSet<String> =
                title_list.iter().map(|(id, _, _)| id.clone()).collect();
            let mut checks: Vec<CheckTarget> = Vec::new();
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
                    None,
                    true,
                );
                checks.push(CheckTarget::Emulator(entry.clone()));
                info_list.push(info);
            }
            checks.append(&mut native_checks);

            // Add pure-cloud entries (server has them, but no local folder).
            // Nothing local to verify, so these never enter `checks`.
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
                        let info = Self::build_sync_info(
                            id,
                            None,
                            &display_name,
                            "",
                            &local_dir,
                            &manifest,
                            None,
                            false,
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
            *cloud_manifest.write().unwrap() = manifest.clone();
            *fetch_at.write().unwrap() = crate::utils::current_time() as u64;
            data_valid.store(true, Ordering::Relaxed);

            // Verify each queued entry's live status, one at a time. Bails
            // quietly, leaving the rest at their provisional status until
            // the next fetch, if a real sync action starts in the meantime
            // rather than fight it for the mount.
            if checks.is_empty() || pending.load(Ordering::Relaxed) {
                return;
            }
            pfs_mount.clear();
            for target in checks {
                if pending.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                    break;
                }
                let (title_id, live_hash) = match target {
                    CheckTarget::Emulator(entry) => {
                        let exclusions = config.psp_exclusions_for(&entry.id);
                        let live_target = entry.save_target_excluding(&exclusions);
                        (entry.id.clone(), content_hash_sources(&live_target.sources))
                    }
                    CheckTarget::Native { title_id, real_id } => {
                        let Some(game_save_dir) = Self::native_save_dir(&real_id) else {
                            // No live save at all: nothing to verify, the
                            // provisional (backup-based) status already
                            // final. Still clear `checking` or the badge
                            // would show "Checking" forever.
                            if let Ok(mut games) = games.write() {
                                if let Some(g) =
                                    games.iter_mut().find(|g| g.title_id == title_id)
                                {
                                    g.checking = false;
                                }
                            }
                            continue;
                        };
                        if !pfs_mount.wait_for_mount(&cancel, &game_save_dir) {
                            break;
                        }
                        (
                            title_id,
                            content_hash_sources(&SaveTarget::single(&game_save_dir).sources),
                        )
                    }
                };
                let cloud_data = manifest.as_ref().and_then(|m| m.games.get(&title_id));
                let last_synced_hash = LocalManifest::load()
                    .games
                    .get(&title_id)
                    .and_then(|e| e.last_synced_hash.clone());
                let status = status_for(
                    true,
                    cloud_data.is_some(),
                    live_hash.as_deref(),
                    cloud_data.and_then(|ce| ce.content_hash.as_deref()),
                    last_synced_hash.as_deref(),
                );
                if let Ok(mut games) = games.write() {
                    if let Some(g) = games.iter_mut().find(|g| g.title_id == title_id) {
                        g.status = status;
                        g.has_local_backup = true;
                        g.checking = false;
                    }
                }
            }
        });
    }

    fn build_sync_info(
        title_id: &str,
        real_id: Option<&str>,
        name: &str,
        server_title: &str,
        local_dir: &str,
        manifest: &Option<CloudManifest>,
        live_hash: Option<String>,
        checking: bool,
    ) -> SyncGameInfo {
        let (backup_exists, local_time, backup_content) = Self::scan_local_backup(local_dir);
        let (has_local, local_content) = match live_hash {
            Some(h) => (true, Some(h)),
            None => (backup_exists, backup_content),
        };
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
            checking,
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

    /// Upload a single game: create a local backup first if it doesn't have
    /// one yet (same as Sync All), then SHA256 and POST it to the server.
    pub fn upload_single(&self, game: &SyncGameInfo) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }

        self.pfs_mount.clear();
        self.cancel.store(false, Ordering::Relaxed);
        let pfs_mount = self.pfs_mount.clone();
        let cancel = Arc::clone(&self.cancel);
        let ts = crate::ime::get_current_format_time().to_string();
        let tid = game.title_id.to_string();
        let n = game.name.to_string();
        let server_title = game.server_title.to_string();
        let game = game.clone();
        let pending = Arc::clone(&self.pending);
        let games = Arc::clone(&self.games);
        let cloud_manifest = Arc::clone(&self.cloud_manifest);

        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let config = Config::global();
            let zip_path = match Self::ensure_backup(&game, &pfs_mount, &cancel) {
                Ok(path) => path,
                Err(err) => {
                    Toast::show(format!("Backup failed: {}", err));
                    Loading::hide();
                    pending.store(false, Ordering::Relaxed);
                    return;
                }
            };
            let hash = match sha256_file(&zip_path) {
                Ok(h) => h,
                Err(_) => {
                    Toast::show("Failed to hash local backup.".to_string());
                    Loading::hide();
                    pending.store(false, Ordering::Relaxed);
                    return;
                }
            };
            let content_hash = read_content_hash_sidecar(&zip_path).unwrap_or_default();
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

    /// Download a single game from server to local backup dir, then restore
    /// it in place (PSP/RetroArch directly, native titles via a PFS mount).
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
        self.pfs_mount.clear();
        self.cancel.store(false, Ordering::Relaxed);
        let pfs_mount = self.pfs_mount.clone();
        let cancel = Arc::clone(&self.cancel);
        let game = game.clone();
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
                    match Self::restore_downloaded(&game, &dl_path, &pfs_mount, &cancel) {
                        Ok(true) => Toast::show(format!("{} downloaded & restored.", n)),
                        Ok(false) => Toast::show(format!("{} downloaded.", n)),
                        Err(e) => {
                            error!("restore {} failed: {}", tid, e);
                            Toast::show(format!("{} downloaded; restore failed: {}", n, e));
                        }
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
        if game.checking {
            Toast::show(status_not_ready("still checking this save"));
            return;
        }
        match game.status {
            SyncStatus::UploadNeeded | SyncStatus::LocalOnly => {
                self.upload_single(game);
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
        if !self.data_valid.load(Ordering::Relaxed) {
            Toast::show(status_not_ready("check your connection"));
            return;
        }

        let games = self.games.read().unwrap().clone();
        if games.iter().any(|g| g.checking) {
            Toast::show(status_not_ready("still checking saves"));
            return;
        }
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
                        // Restores in place (PSP/RetroArch directly, native
                        // via PFS mount), auto-backing-up the live save first.
                        if let Err(e) =
                            Self::restore_downloaded(game, &dl_path, &pfs_mount, &cancel)
                        {
                            error!("restore {} failed: {}", game.title_id, e);
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

    /// Always makes a fresh local backup from the live save data and
    /// returns its path — never reuses an older zip, which could predate
    /// the change that made this upload necessary in the first place.
    fn ensure_backup(
        game: &SyncGameInfo,
        pfs_mount: &PfsMountHandshake,
        cancel: &Arc<AtomicBool>,
    ) -> Result<String, String> {
        let backup_to_path = format!(
            "{}/{}.zip",
            game.local_dir,
            crate::ime::get_current_format_time()
        );
        if let Some(real_id) = &game.real_id {
            let game_save_dir =
                Self::native_save_dir(real_id).ok_or_else(|| "no save data".to_string())?;
            if !pfs_mount.wait_for_mount(cancel, &game_save_dir) {
                return Err("cancelled".to_string());
            }
            backup_game_save(&game_save_dir, &backup_to_path).map_err(|e| format!("{:?}", e))?;
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

    /// A native title's live save directory, whichever of the card/internal
    /// paths actually exists.
    fn native_save_dir(real_id: &str) -> Option<String> {
        [
            format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
            format!("{}/{}", GAME_SAVE_DIR, real_id),
        ]
        .into_iter()
        .find(|dir| Path::new(dir).exists())
    }

    /// Restore a just-downloaded archive in place: PSP/RetroArch saves
    /// restore directly, native titles restore via a PFS-mounted SaveTarget
    /// built from their real_id. `Ok(false)` means there was nothing to
    /// restore into (not an error, just download-only).
    fn restore_downloaded(
        game: &SyncGameInfo,
        dl_path: &str,
        pfs_mount: &PfsMountHandshake,
        cancel: &Arc<AtomicBool>,
    ) -> Result<bool, String> {
        if let Some(target) = save_target_for_downloaded_archive(&game.title_id, dl_path) {
            restore_save_target(&target, dl_path).map_err(|e| e.to_string())?;
            return Ok(true);
        }
        if let Some(real_id) = &game.real_id {
            if let Some(game_save_dir) = Self::native_save_dir(real_id) {
                if !pfs_mount.wait_for_mount(cancel, &game_save_dir) {
                    return Err("cancelled".to_string());
                }
                let target = SaveTarget::single(&game_save_dir);
                restore_save_target(&target, dl_path).map_err(|e| e.to_string())?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn status_rgb(status: &SyncStatus) -> (i32, i32, i32) {
        match status {
            SyncStatus::InSync => (0x44, 0xcc, 0x44),
            SyncStatus::UploadNeeded | SyncStatus::LocalOnly => (0x44, 0x88, 0xff),
            SyncStatus::DownloadAvailable | SyncStatus::CloudOnly => (0xff, 0xaa, 0x44),
            SyncStatus::Conflict => (0xff, 0x44, 0x44),
        }
    }

    /// Same palette, custom alpha — for overlays drawn on top of other
    /// content (e.g. a badge sitting over a game icon).
    pub fn status_color_alpha(status: &SyncStatus, a: i32) -> u32 {
        let (r, g, b) = Self::status_rgb(status);
        rgba(r, g, b, a)
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
