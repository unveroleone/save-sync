use std::{
    ffi::OsStr,
    fmt::{Display, Formatter},
    fs,
    ops::Deref,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
};

use log::error;

use crate::{
    api::Api,
    config::Config,
    constant::{CANCEL_HINT, GAME_CARD_SAVE_DIR, GAME_SAVE_DIR, SCREEN_WIDTH},
    emulator::{scan_emulator_entries, EmulatorEntry, EmulatorKind},
    ime::get_current_format_time,
    tai::{mount_pfs, psv_launch_app_by_title_id, unmount_pfs, Title, Titles},
    ui::{
        ui_cloud::list_state::ListState, ui_dialog::UIDialog, ui_loading::Loading, ui_toast::Toast,
    },
    utils::{
        backup_game_save, backup_save_target, get_active_color, get_game_local_backup_dir,
        read_content_hash_sidecar, sha256_file, update_sfo_file_with_current_account_id,
    },
    vita2d::{is_button, rgba, vita2d_draw_rect, vita2d_draw_text, SceCtrlButtons},
};

enum GameMenuAction {
    LaunchApp,
    BackupAllGameSave,
    BackupAllToServer,
    UpdateAccountId,
    DeleteGameSave,
    DeleteSelectedGameSave,
    DeleteAllGameSaves,
    SelectFolders,
}

impl Deref for GameMenuAction {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            GameMenuAction::LaunchApp => "Launch Game",
            GameMenuAction::BackupAllGameSave => "Backup All Game Saves",
            GameMenuAction::BackupAllToServer => "Backup All to Server",
            GameMenuAction::UpdateAccountId => "Update Account ID",
            GameMenuAction::DeleteGameSave => "Delete Game Save",
            GameMenuAction::DeleteSelectedGameSave => "Delete Local Backup",
            GameMenuAction::DeleteAllGameSaves => "Delete All Local Backups",
            GameMenuAction::SelectFolders => "Select Folders",
        }
    }
}

impl Display for GameMenuAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.deref())
    }
}

/// The loading dialog only draws when a desc is set, so bulk progress has to
/// report both or the user sees nothing but a spinner.
fn notify_bulk_progress(action: &str, idx: usize, total: usize, name: &str) {
    Loading::notify_title(format!("{} ({}/{})    {}", action, idx, total, CANCEL_HINT));
    Loading::notify_desc(name.to_string());
}

/// Closing message for a bulk run, so a stopped run never reads as a finished
/// one and the counts always say what actually happened.
fn bulk_result_message(verb: &str, done: usize, failed: usize, cancelled: bool) -> String {
    let head = if cancelled { "Stopped" } else { "Done" };
    if failed == 0 {
        format!("{}: {} save(s) {}.", head, done, verb)
    } else {
        format!("{}: {} {}, {} failed.", head, done, verb, failed)
    }
}

/// Park until the main thread mounts `game_save_dir`. Only the main thread may
/// mount, so the worker asks and waits. Returns false when the run was
/// cancelled while waiting, in which case nothing was mounted and the caller
/// must not touch the save.
fn wait_for_mount(
    on_mounted: &Arc<RwLock<Option<String>>>,
    prepare_to_mount: &Arc<RwLock<Option<String>>>,
    cancel: &Arc<AtomicBool>,
    game_save_dir: &str,
) -> bool {
    let mut is_prepare = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        if let Ok(mounted) = on_mounted.try_read() {
            if let Some(mounted) = mounted.as_ref() {
                if mounted == game_save_dir {
                    return true;
                }
            }
        }
        if !is_prepare {
            if let Ok(mut prepare) = prepare_to_mount.try_write() {
                is_prepare = true;
                *prepare = Some(game_save_dir.to_string());
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Hash and upload one finished backup. Failures are logged and reported to the
/// caller so a bulk run can keep going. `title` labels the save on the server
/// (empty skips the label).
fn upload_backup(config: &Config, title_id: &str, title: &str, backup_path: &str) -> bool {
    let hash = match sha256_file(backup_path) {
        Ok(hash) => hash,
        Err(err) => {
            error!("hash {} failed: {:?}", backup_path, err);
            return false;
        }
    };
    let timestamp = get_current_format_time();
    let content_hash = read_content_hash_sidecar(backup_path).unwrap_or_default();
    match Api::upload_save(
        config,
        title_id,
        title,
        &content_hash,
        backup_path,
        &hash,
        &timestamp,
    ) {
        Ok(_) => {
            crate::sync::LocalManifest::record(title_id, &content_hash);
            true
        }
        Err(err) => {
            error!("upload {} failed: {}", title_id, err);
            false
        }
    }
}

/// Whose save the menu was opened for. Native titles get the full action set,
/// while emulator entries (PSP/RetroArch) get the subset that makes sense for
/// them: backup, backup+upload, and local-backup deletion.
enum GameListMode {
    Native,
    Emulator(EmulatorEntry),
}

pub struct GameList {
    pending: Arc<AtomicBool>,
    list_state: ListState,
    list: Vec<GameMenuAction>,
    mode: GameListMode,
    /// Active folder picker: (folder name, included). PSP entries with more
    /// than one folder can exclude install/DLC data from backups.
    folder_picker: Option<Vec<(String, bool)>>,
    game_save_dir_prepare_to_mount: Arc<RwLock<Option<String>>>,
    game_save_dir_on_mounted: Arc<RwLock<Option<String>>>,
    cancel: Arc<AtomicBool>,
}

impl GameList {
    pub fn new() -> Self {
        let mut list = GameList {
            pending: Arc::new(AtomicBool::new(false)),
            list_state: ListState::new(15),
            list: Vec::new(),
            mode: GameListMode::Native,
            folder_picker: None,
            game_save_dir_prepare_to_mount: Arc::new(RwLock::new(None)),
            game_save_dir_on_mounted: Arc::new(RwLock::new(None)),
            cancel: Arc::new(AtomicBool::new(false)),
        };
        list.set_native();
        list
    }

    /// Native action set, including the whole-device bulk operations.
    pub fn set_native(&mut self) {
        self.mode = GameListMode::Native;
        self.list = vec![
            GameMenuAction::LaunchApp,
            GameMenuAction::BackupAllGameSave,
            GameMenuAction::BackupAllToServer,
            GameMenuAction::UpdateAccountId,
            GameMenuAction::DeleteGameSave,
            GameMenuAction::DeleteSelectedGameSave,
            GameMenuAction::DeleteAllGameSaves,
        ];
        self.folder_picker = None;
        self.list_state.reset();
    }

    /// Emulator action set. LaunchApp and UpdateAccountId are native-only, and
    /// the whole-device delete operations would surprise from a single PSP or
    /// RetroArch entry.
    pub fn set_emulator(&mut self, entry: &EmulatorEntry) {
        self.mode = GameListMode::Emulator(entry.clone());
        let mut actions = vec![
            GameMenuAction::BackupAllGameSave,
            GameMenuAction::BackupAllToServer,
            GameMenuAction::DeleteSelectedGameSave,
        ];
        // The folder picker only makes sense when a PSP game owns several
        // folders (save slots + DLC/install data).
        if entry.kind == EmulatorKind::Psp && entry.all_paths().len() > 1 {
            actions.push(GameMenuAction::SelectFolders);
        }
        self.list = actions;
        self.folder_picker = None;
        self.list_state.reset();
    }

    pub fn picker_active(&self) -> bool {
        self.folder_picker.is_some()
    }

    /// Ask a running bulk operation to stop. There is no confirmation dialog:
    /// UIDialog runs its own loop on the main thread, which is the same thread
    /// the worker depends on for mounting, so blocking here would stall it.
    fn request_cancel(&self) {
        if !self.cancel.swap(true, Ordering::Relaxed) {
            Toast::show("Stopping after this game...".to_string());
        }
    }

    pub fn is_pending(&self) -> bool {
        self.pending.load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn delete_game_save(&self, title: &Title) {
        let real_id = title.real_id().to_string();
        let name = title.name().to_string();
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        unmount_pfs();
        self.clear_mounted_state();
        tokio::spawn(async move {
            let dirs = [
                format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
                format!("{}/{}", GAME_SAVE_DIR, real_id),
            ];
            if let Some(game_save_dir) = dirs.iter().find(|dir| Path::new(&dir).exists()) {
                if let Err(err) = fs::remove_dir_all(&game_save_dir) {
                    error!("remove {} failed: {}", game_save_dir, err);
                    Toast::show(format!("Failed to delete {} save!", name));
                } else {
                    Toast::show(format!("Deleted {} save!", name));
                }
            } else {
                Toast::show(format!("{} save not found!", name));
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    pub fn delete_selected_game_save(&self, title: &Title) {
        let title_id = title.title_id().to_string();
        let name = title.name().to_string();
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let local_dir = get_game_local_backup_dir(&title_id, &name);
            if Path::new(&local_dir).exists() {
                if let Err(err) = fs::remove_dir_all(&local_dir) {
                    error!("remove {} failed: {}", local_dir, err);
                    Toast::show(format!("Failed to delete {} local backup!", name));
                } else {
                    Toast::show(format!("Deleted {} local backup!", name));
                }
            } else {
                Toast::show(format!("{} local backup not found!", name));
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    pub fn delete_all_game_saves(&self, titles: &Titles) {
        let list = titles
            .iter()
            .map(|title| (title.title_id().to_string(), title.name().to_string()))
            .collect::<Vec<(String, String)>>();

        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let mut delete_failed_count = 0;
            for (_idx, (title_id, name)) in list.iter().enumerate() {
                let local_dir = get_game_local_backup_dir(&title_id, &name);
                if Path::new(&local_dir).exists() {
                    if let Err(err) = fs::remove_dir_all(&local_dir) {
                        error!("remove {} failed: {}", local_dir, err);
                        Toast::show(format!("Failed to delete {} backup!", name));
                        delete_failed_count += 1;
                    }
                }
            }
            if delete_failed_count == 0 {
                Toast::show("All backups deleted!".to_string());
            } else {
                Toast::show(format!("{} deletions failed!", delete_failed_count));
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    pub fn backup_all_game_save(&self, titles: &Titles) {
        let list = titles
            .iter()
            .map(|title| {
                (
                    title.title_id().to_string(),
                    title.real_id().to_string(),
                    title.name().to_string(),
                )
            })
            .collect::<Vec<(String, String, String)>>();

        // Start from a known state so the first game always gets a fresh mount.
        self.clear_mounted_state();
        self.cancel.store(false, Ordering::Relaxed);
        let game_save_dir_on_mounted = Arc::clone(&self.game_save_dir_on_mounted);
        let game_save_dir_prepare_to_mount = Arc::clone(&self.game_save_dir_prepare_to_mount);
        let cancel = Arc::clone(&self.cancel);
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let mut done = 0;
            let mut backup_failed_count = 0;
            let mut cancelled = false;
            for (idx, (title_id, real_id, name)) in list.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                notify_bulk_progress("Backing up", idx + 1, list.len(), name);
                let dirs = [
                    format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
                    format!("{}/{}", GAME_SAVE_DIR, real_id),
                ];
                let game_save_dir = match dirs.iter().find(|dir| Path::new(&dir).exists()) {
                    Some(dir) => dir.to_string(),
                    None => continue,
                };
                if !wait_for_mount(
                    &game_save_dir_on_mounted,
                    &game_save_dir_prepare_to_mount,
                    &cancel,
                    &game_save_dir,
                ) {
                    cancelled = true;
                    break;
                }
                let backup_to_path = format!(
                    "{}/{}.zip",
                    get_game_local_backup_dir(&title_id, &name),
                    get_current_format_time()
                );
                match backup_game_save(&game_save_dir, &backup_to_path) {
                    Ok(_) => done += 1,
                    Err(err) => {
                        backup_failed_count += 1;
                        error!(
                            "zip {} to {} failed: {:?}",
                            game_save_dir, backup_to_path, err
                        );
                        Toast::show(format!("Backup failed for {}!", name));
                    }
                }
            }
            Toast::show(bulk_result_message(
                "backed up",
                done,
                backup_failed_count,
                cancelled,
            ));
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Back up every save and push it to the server in one pass. Mirrors
    /// backup_all_game_save, including its main-thread mount handshake, and
    /// covers emulator entries too so the whole device lands on the server.
    pub fn backup_all_to_server(&self, titles: &Titles) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }

        let list = titles
            .iter()
            .map(|title| {
                (
                    title.title_id().to_string(),
                    title.real_id().to_string(),
                    title.name().to_string(),
                )
            })
            .collect::<Vec<(String, String, String)>>();
        let emulator_entries = scan_emulator_entries();
        let total = list.len() + emulator_entries.len();

        if total == 0 {
            Toast::show("No saves to upload!".to_string());
            return;
        }

        // Dialogs run their own render loop, so confirm before spawning.
        if !UIDialog::present(&format!("Back up and upload {} save(s) to server?", total)) {
            return;
        }

        // Start from a known state so the first game always gets a fresh mount.
        self.clear_mounted_state();
        self.cancel.store(false, Ordering::Relaxed);
        let game_save_dir_on_mounted = Arc::clone(&self.game_save_dir_on_mounted);
        let game_save_dir_prepare_to_mount = Arc::clone(&self.game_save_dir_prepare_to_mount);
        let cancel = Arc::clone(&self.cancel);
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let mut uploaded = 0;
            let mut failed = 0;
            let mut idx = 0;
            let mut cancelled = false;

            for (title_id, real_id, name) in list.iter() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                idx += 1;
                notify_bulk_progress("Uploading", idx, total, name);

                let dirs = [
                    format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
                    format!("{}/{}", GAME_SAVE_DIR, real_id),
                ];
                let game_save_dir = match dirs.iter().find(|dir| Path::new(&dir).exists()) {
                    Some(dir) => dir.to_string(),
                    None => continue,
                };

                if !wait_for_mount(
                    &game_save_dir_on_mounted,
                    &game_save_dir_prepare_to_mount,
                    &cancel,
                    &game_save_dir,
                ) {
                    cancelled = true;
                    break;
                }

                let backup_to_path = format!(
                    "{}/{}.zip",
                    get_game_local_backup_dir(title_id, name),
                    get_current_format_time()
                );
                if let Err(err) = backup_game_save(&game_save_dir, &backup_to_path) {
                    failed += 1;
                    error!(
                        "zip {} to {} failed: {:?}",
                        game_save_dir, backup_to_path, err
                    );
                    continue;
                }
                if upload_backup(&config, title_id, name, &backup_to_path) {
                    uploaded += 1;
                } else {
                    failed += 1;
                }
            }

            // Emulator saves need no PFS mount.
            for entry in emulator_entries.iter() {
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    break;
                }
                idx += 1;
                notify_bulk_progress("Uploading", idx, total, &entry.name);

                let backup_to_path = format!(
                    "{}/{}.zip",
                    entry.local_backup_dir(),
                    get_current_format_time()
                );
                let exclusions = Config::global().psp_exclusions_for(&entry.id);
                if let Err(err) =
                    backup_save_target(&entry.save_target_excluding(&exclusions), &backup_to_path)
                {
                    failed += 1;
                    error!("zip {} to {} failed: {:?}", entry.id, backup_to_path, err);
                    continue;
                }
                if upload_backup(&config, &entry.id, &entry.server_title, &backup_to_path) {
                    uploaded += 1;
                } else {
                    failed += 1;
                }
            }

            Toast::show(bulk_result_message("uploaded", uploaded, failed, cancelled));
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Back up a single emulator entry (PSP game / RetroArch save) to its
    /// local backup directory. No PFS mount: emulator saves live on plain
    /// ux0 paths.
    fn backup_emulator_to_local(&self, entry: &EmulatorEntry) {
        let entry = entry.clone();
        let exclusions = Config::global().psp_exclusions_for(&entry.id);
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let backup_to_path = format!(
                "{}/{}.zip",
                entry.local_backup_dir(),
                get_current_format_time()
            );
            match backup_save_target(&entry.save_target_excluding(&exclusions), &backup_to_path) {
                Ok(_) => Toast::show(format!("{} backed up.", entry.name)),
                Err(err) => {
                    error!("zip {} to {} failed: {:?}", entry.id, backup_to_path, err);
                    Toast::show(format!("Backup failed for {}!", entry.name));
                }
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Back up a single emulator entry and push it to the server.
    fn backup_emulator_to_server(&self, entry: &EmulatorEntry) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }
        let entry = entry.clone();
        let exclusions = Config::global().psp_exclusions_for(&entry.id);
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let backup_to_path = format!(
                "{}/{}.zip",
                entry.local_backup_dir(),
                get_current_format_time()
            );
            match backup_save_target(&entry.save_target_excluding(&exclusions), &backup_to_path) {
                Ok(_) => {
                    if !upload_backup(&config, &entry.id, &entry.server_title, &backup_to_path) {
                        Toast::show(format!("Upload failed for {}!", entry.name));
                    } else {
                        Toast::show(format!("{} uploaded.", entry.name));
                    }
                }
                Err(err) => {
                    error!("zip {} to {} failed: {:?}", entry.id, backup_to_path, err);
                    Toast::show(format!("Backup failed for {}!", entry.name));
                }
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Delete every local backup of one emulator entry.
    fn delete_emulator_backups(&self, entry: &EmulatorEntry) {
        let entry = entry.clone();
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let local_dir = entry.local_backup_dir();
            if Path::new(&local_dir).exists() {
                if let Err(err) = fs::remove_dir_all(&local_dir) {
                    error!("remove {} failed: {}", local_dir, err);
                    Toast::show(format!("Failed to delete {} local backup!", entry.name));
                } else {
                    Toast::show(format!("Deleted {} local backup!", entry.name));
                }
            } else {
                Toast::show(format!("{} local backup not found!", entry.name));
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Forget which save is mounted. Anything that unmounts must call this, or
    /// a later bulk run sees a stale match, skips its mount request and
    /// archives an unmounted directory.
    fn clear_mounted_state(&self) {
        *self.game_save_dir_on_mounted.write().unwrap() = None;
        *self.game_save_dir_prepare_to_mount.write().unwrap() = None;
    }

    pub fn mount_game_dir_if_exists(&self) {
        let prepare_dir = match self.game_save_dir_prepare_to_mount.try_write() {
            Ok(mut prepare_dir) => {
                if prepare_dir.is_none() {
                    None
                } else {
                    Some(prepare_dir.take().unwrap())
                }
            }
            _ => None,
        };

        if let Some(prepare_dir) = prepare_dir {
            mount_pfs(&prepare_dir);
            *self.game_save_dir_on_mounted.write().unwrap() = Some(prepare_dir);
        }
    }

    pub fn update(
        &mut self,
        buttons: u32,
        title: Option<&Title>,
        titles: &Titles,
        emu: Option<&EmulatorEntry>,
    ) {
        self.mount_game_dir_if_exists();

        if self.is_pending() {
            // A bulk run holds all input, so circle is free to mean "stop".
            if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
                self.request_cancel();
            }
            return;
        }

        // Folder picker mode: cross toggles a folder, circle saves and
        // returns to the action list. The game menu defers its circle-close
        // while the picker is active (see GameMenu::update).
        if self.folder_picker.is_some() {
            let mut picker = self.folder_picker.take().unwrap();
            if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
                let excluded: Vec<String> = picker
                    .iter()
                    .filter(|(_, included)| !*included)
                    .map(|(name, _)| name.clone())
                    .collect();
                if let GameListMode::Emulator(entry) = &self.mode {
                    let mut config = Config::global();
                    config.set_psp_exclusions(&entry.id, excluded);
                    config.save();
                    Config::update_global(config);
                }
            } else {
                if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
                    if let Some(row) = picker.get_mut(self.list_state.selected_idx as usize) {
                        row.1 = !row.1;
                    }
                }
                self.list_state.update(picker.len() as i32, buttons);
                self.folder_picker = Some(picker);
            }
            return;
        }

        let ListState { selected_idx, .. } = self.list_state;
        if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
            match &self.mode {
                GameListMode::Emulator(entry) => {
                    // The caller keeps mode and selection in step; bail out if
                    // they ever disagree instead of acting on the wrong game.
                    if emu.is_none() {
                        return;
                    }
                    let action = &self.list[selected_idx as usize];
                    match action {
                        GameMenuAction::BackupAllGameSave => {
                            if UIDialog::present(&GameMenuAction::BackupAllGameSave) {
                                self.backup_emulator_to_local(entry);
                            }
                        }
                        GameMenuAction::BackupAllToServer => {
                            if UIDialog::present(&GameMenuAction::BackupAllToServer) {
                                self.backup_emulator_to_server(entry);
                            }
                        }
                        GameMenuAction::DeleteSelectedGameSave => {
                            let mut count = 3;
                            loop {
                                if UIDialog::present(&if count == 0 {
                                    format!("{}", GameMenuAction::DeleteSelectedGameSave)
                                } else {
                                    format!(
                                        "{}: {}",
                                        GameMenuAction::DeleteSelectedGameSave, count
                                    )
                                }) {
                                    if count == 0 {
                                        self.delete_emulator_backups(entry);
                                        break;
                                    } else {
                                        count -= 1;
                                    }
                                } else {
                                    break;
                                }
                            }
                        }
                        GameMenuAction::SelectFolders => {
                            let exclusions = Config::global().psp_exclusions_for(&entry.id);
                            let picker: Vec<(String, bool)> = entry
                                .all_paths()
                                .iter()
                                .map(|p| {
                                    let name = Path::new(p)
                                        .file_name()
                                        .unwrap_or(OsStr::new(""))
                                        .to_string_lossy()
                                        .to_string();
                                    let included = !exclusions.iter().any(|ex| ex == &name);
                                    (name, included)
                                })
                                .collect();
                            self.list_state.reset();
                            self.folder_picker = Some(picker);
                        }
                        _ => {}
                    }
                }
                GameListMode::Native => {
                    let title = match title {
                        Some(title) => title,
                        None => return,
                    };
                    let action = &self.list[selected_idx as usize];
                    match action {
                GameMenuAction::LaunchApp => {
                    if UIDialog::present(&format!(
                        "{}: {}",
                        &GameMenuAction::LaunchApp,
                        title.name()
                    )) {
                        psv_launch_app_by_title_id(title.title_id());
                    }
                }
                GameMenuAction::BackupAllGameSave => {
                    if UIDialog::present(&GameMenuAction::BackupAllGameSave) {
                        self.backup_all_game_save(titles);
                    }
                }
                GameMenuAction::BackupAllToServer => {
                    self.backup_all_to_server(titles);
                }
                GameMenuAction::UpdateAccountId => {
                    if UIDialog::present(&GameMenuAction::UpdateAccountId) {
                        [
                            format!("{}/{}", GAME_CARD_SAVE_DIR, title.real_id()),
                            format!("{}/{}", GAME_SAVE_DIR, title.real_id()),
                        ]
                        .iter()
                        .any(|path| {
                            let sfo_path = format!("{}/sce_sys/param.sfo", path);
                            if Path::new(&sfo_path).exists() {
                                mount_pfs(path);
                                if let Ok(()) = update_sfo_file_with_current_account_id(&sfo_path)
                                {
                                    Toast::show("Account ID updated!".to_string());
                                } else {
                                    Toast::show("Account ID update failed!".to_string());
                                }
                                unmount_pfs();
                                self.clear_mounted_state();
                                return true;
                            }
                            false
                        });
                    }
                }
                GameMenuAction::DeleteGameSave => {
                    let mut count = 3;
                    loop {
                        if UIDialog::present(&if count == 0 {
                            format!("{}", GameMenuAction::DeleteGameSave)
                        } else {
                            format!("{}: {}", GameMenuAction::DeleteGameSave, count)
                        }) {
                            if count == 0 {
                                self.delete_game_save(title);
                                break;
                            } else {
                                count -= 1;
                            }
                        } else {
                            break;
                        }
                    }
                }
                GameMenuAction::DeleteSelectedGameSave => {
                    let mut count = 3;
                    loop {
                        if UIDialog::present(&if count == 0 {
                            format!("{}", GameMenuAction::DeleteSelectedGameSave)
                        } else {
                            format!("{}: {}", GameMenuAction::DeleteSelectedGameSave, count)
                        }) {
                            if count == 0 {
                                self.delete_selected_game_save(title);
                                break;
                            } else {
                                count -= 1;
                            }
                        } else {
                            break;
                        }
                    }
                }
                GameMenuAction::DeleteAllGameSaves => {
                    let mut count = 3;
                    loop {
                        if UIDialog::present(&if count == 0 {
                            format!("{}", GameMenuAction::DeleteAllGameSaves)
                        } else {
                            format!("{}: {}", GameMenuAction::DeleteAllGameSaves, count)
                        }) {
                            if count == 0 {
                                self.delete_all_game_saves(titles);
                                break;
                            } else {
                                count -= 1;
                            }
                        } else {
                            break;
                        }
                    }
                }
                // PSP-only action; never part of the native list.
                GameMenuAction::SelectFolders => {}
            }
                }
            }
        }

        self.list_state.update(self.list.len() as i32, buttons);
    }

    pub fn draw(&self, left: i32, top: i32) {
        // Folder picker mode: checkbox rows instead of the action list.
        if let Some(picker) = &self.folder_picker {
            let ListState {
                top_row,
                selected_idx,
                display_row,
            } = self.list_state;
            for idx in 0..display_row {
                let i = top_row + idx;
                if i >= picker.len() as i32 {
                    break;
                }
                let (name, included) = &picker[i as usize];
                let x = left + 12;
                let y = top + 22 + 14;
                if i == selected_idx {
                    vita2d_draw_rect(
                        x as f32,
                        (y + 30 * idx - 22) as f32,
                        (SCREEN_WIDTH / 2 - 24) as f32,
                        30.0,
                        get_active_color(),
                    );
                    vita2d_draw_rect(
                        (x + 2) as f32,
                        (y + 2 + 30 * idx - 22) as f32,
                        (SCREEN_WIDTH / 2 - 28) as f32,
                        26.0,
                        rgba(0x18, 0x18, 0x18, 0xff),
                    );
                }
                let label = format!("{} {}", if *included { "[x]" } else { "[ ]" }, name);
                vita2d_draw_text(
                    x + 8,
                    y + 30 * idx,
                    rgba(0xff, 0xff, 0xff, 0xff),
                    1.0,
                    &label,
                );
            }
            return;
        }

        let actions = &self.list;
        let size = actions.len() as i32;
        let ListState {
            top_row,
            selected_idx,
            display_row,
        } = self.list_state;
        for idx in 0..display_row {
            let i = top_row + idx;
            if i >= size {
                break;
            }
            let x = left + 12;
            let y = top + 22 + 14;
            if i == selected_idx {
                vita2d_draw_rect(
                    x as f32,
                    (y + 30 * idx - 22) as f32,
                    (SCREEN_WIDTH / 2 - 24) as f32,
                    30.0,
                    get_active_color(),
                );
                vita2d_draw_rect(
                    (x + 2) as f32,
                    (y + 2 + 30 * idx - 22) as f32,
                    (SCREEN_WIDTH / 2 - 28) as f32,
                    26.0,
                    rgba(0x18, 0x18, 0x18, 0xff),
                );
            }

            vita2d_draw_text(
                x + 8,
                y + 30 * idx,
                rgba(0xff, 0xff, 0xff, 0xff),
                1.0,
                &actions[i as usize],
            );
        }
    }
}
