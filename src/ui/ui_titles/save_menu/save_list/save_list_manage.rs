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
    api::{Api, CloudGameEntry},
    config::Config,
    constant::{GAME_CARD_SAVE_DIR, GAME_SAVE_DIR, SCREEN_WIDTH},
    emulator::{EmulatorEntry, EmulatorKind},
    ime::get_current_format_time,
    sync::LocalManifest,
    tai::{mount_pfs, psv_launch_app_by_title_id, unmount_pfs},
    ui::{
        list_state::ListState, ui_dialog::UIDialog, ui_list::UIList, ui_loading::Loading,
        ui_toast::Toast,
    },
    utils::{
        backup_save_target, delete_dir_if_empty, get_active_color, get_game_local_backup_dir,
        read_content_hash_sidecar, restore_save_target, sha256_file,
        update_sfo_file_with_current_account_id, SaveTarget,
    },
    vita2d::{is_button, rgba, vita2d_draw_rect, vita2d_draw_text, SceCtrlButtons},
};

use super::DISPLAY_ROW;

/// Whether this entry is a native Vita title or an emulator (PSP/RetroArch)
/// entry. Native titles get a few extra rows (launch, account ID, live save
/// deletion) that only make sense for them.
pub enum ManageContext {
    Native { real_id: String },
    Emulator(EmulatorEntry),
}

enum ManageAction {
    UploadToServer,
    RestoreFromServer,
    DeleteFromServer,
    LaunchApp,
    UpdateAccountId,
    DeleteGameSave,
    DeleteSelectedGameSave,
    SelectFolders,
    ToggleSyncExclusion,
}

impl Deref for ManageAction {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            ManageAction::UploadToServer => "Upload to Server",
            ManageAction::RestoreFromServer => "Restore from Server",
            ManageAction::DeleteFromServer => "Delete Server Backup",
            ManageAction::LaunchApp => "Launch Game",
            ManageAction::UpdateAccountId => "Update Account ID",
            ManageAction::DeleteGameSave => "Delete Live Save",
            ManageAction::DeleteSelectedGameSave => "Delete Local Backup",
            ManageAction::SelectFolders => "Select Folders",
            ManageAction::ToggleSyncExclusion => "Exclude from Sync",
        }
    }
}

impl Display for ManageAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.deref())
    }
}

pub struct SaveListManage {
    pending: Arc<AtomicBool>,
    list_state: ListState,
    list: Vec<ManageAction>,
    local_dir: String,
    title_id: String,
    title_name: String,
    server_title: String,
    needs_pfs: bool,
    context: ManageContext,
    cloud_entry: Arc<RwLock<Option<CloudGameEntry>>>,
    /// Active folder picker: (folder name, included). PSP entries with more
    /// than one folder can exclude install/DLC data from backups.
    folder_picker: Option<Vec<(String, bool)>>,
    sync_exclusion_changed: bool,
}

impl SaveListManage {
    pub fn new(
        title_id: &str,
        title_name: &str,
        server_title: &str,
        needs_pfs: bool,
        context: ManageContext,
    ) -> SaveListManage {
        let mut list = vec![
            ManageAction::UploadToServer,
            ManageAction::RestoreFromServer,
            ManageAction::DeleteFromServer,
        ];
        match &context {
            ManageContext::Native { .. } => {
                list.push(ManageAction::LaunchApp);
                list.push(ManageAction::UpdateAccountId);
                list.push(ManageAction::DeleteGameSave);
                list.push(ManageAction::DeleteSelectedGameSave);
            }
            ManageContext::Emulator(entry) => {
                list.push(ManageAction::DeleteSelectedGameSave);
                // The folder picker only makes sense when a PSP game owns
                // several folders (save slots + DLC/install data).
                if entry.kind == EmulatorKind::Psp && entry.all_paths().len() > 1 {
                    list.push(ManageAction::SelectFolders);
                }
            }
        }
        list.push(ManageAction::ToggleSyncExclusion);

        SaveListManage {
            list_state: ListState::new(DISPLAY_ROW),
            pending: Arc::new(AtomicBool::new(false)),
            list,
            local_dir: get_game_local_backup_dir(title_id, title_name),
            title_id: title_id.to_string(),
            title_name: title_name.to_string(),
            server_title: server_title.to_string(),
            needs_pfs,
            context,
            cloud_entry: Arc::new(RwLock::new(None)),
            folder_picker: None,
            sync_exclusion_changed: false,
        }
    }

    fn entry_id(&self) -> String {
        match &self.context {
            ManageContext::Native { .. } => self.title_id.clone(),
            ManageContext::Emulator(entry) => entry.id.clone(),
        }
    }

    fn fetch_cloud_entry(&self) {
        let title_id = self.title_id.clone();
        let cloud_entry = Arc::clone(&self.cloud_entry);
        let config = Config::global();
        if !config.is_configured() {
            return;
        }
        tokio::spawn(async move {
            match Api::get_cloud_manifest(&config) {
                Ok(manifest) => {
                    if let Some(entry) = manifest.games.get(&title_id) {
                        *cloud_entry.write().unwrap() = Some(entry.clone());
                    }
                }
                Err(e) => {
                    error!("fetch manifest failed: {}", e);
                }
            }
        });
    }

    fn upload_to_server(&self, save_target: &Option<SaveTarget>) {
        let save_target = match save_target {
            Some(target) => target.clone(),
            None => {
                Toast::show("No game save found!".to_string());
                return;
            }
        };
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }

        let local_dir = self.local_dir.clone();
        let backup_path = format!("{}/{}.zip", local_dir, get_current_format_time());
        let title_id = self.title_id.clone();
        let server_title = self.server_title.clone();
        let cloud_entry = Arc::clone(&self.cloud_entry);

        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        if self.needs_pfs {
            mount_pfs(&save_target.restore_root);
        }
        tokio::spawn(async move {
            Loading::notify_title("Backing up & uploading...".to_string());
            match backup_save_target(&save_target, &backup_path) {
                Ok(_) => {
                    let hash = match sha256_file(&backup_path) {
                        Ok(h) => h,
                        Err(e) => {
                            error!("hash failed: {:?}", e);
                            Toast::show("Hash failed.".to_string());
                            Loading::hide();
                            pending.store(false, Ordering::Relaxed);
                            return;
                        }
                    };
                    let timestamp = get_current_format_time();
                    let content_hash = read_content_hash_sidecar(&backup_path).unwrap_or_default();
                    match Api::upload_save(
                        &config,
                        &title_id,
                        &server_title,
                        &content_hash,
                        &backup_path,
                        &hash,
                        &timestamp,
                    ) {
                        Ok(_) => {
                            if let Ok(manifest) = Api::get_cloud_manifest(&config) {
                                if let Some(entry) = manifest.games.get(&title_id) {
                                    *cloud_entry.write().unwrap() = Some(entry.clone());
                                }
                            }
                            LocalManifest::record(&title_id, &content_hash);
                            Toast::show("Upload complete!".to_string());
                        }
                        Err(e) => {
                            error!("upload failed: {}", e);
                            Toast::show(format!("Upload failed: {}", e));
                        }
                    }
                }
                Err(e) => {
                    error!("backup failed: {:?}", e);
                    Toast::show(format!("Backup failed: {:?}", e));
                }
            }
            if Path::new(&backup_path).exists() {
                let _ = fs::remove_file(&backup_path);
                let _ = delete_dir_if_empty(&local_dir);
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    fn restore_from_server(&self, save_target: &Option<SaveTarget>) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }
        let entry = match self.cloud_entry.read().unwrap().clone() {
            Some(e) => e,
            None => {
                Toast::show("No server backup available.".to_string());
                return;
            }
        };
        if !UIDialog::present(&format!(
            "Restore from server?\n{} ({})",
            entry.latest_version,
            format_size(entry.size)
        )) {
            return;
        }

        let title_id = self.title_id.clone();
        let local_dir = self.local_dir.clone();
        let cloud_content = entry.content_hash.clone();
        let save_target = save_target.clone();
        let needs_pfs = self.needs_pfs;
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();

        tokio::spawn(async move {
            Loading::notify_title("Downloading & restoring...".to_string());
            let dl_path = format!("{}/{}.zip", local_dir, get_current_format_time());
            match Api::download_save(&config, &title_id, &dl_path) {
                Ok(_) => {
                    if let Some(ch) = cloud_content {
                        let _ = std::fs::write(format!("{}.chash", dl_path), &ch);
                        LocalManifest::record(&title_id, &ch);
                    }
                    if let Some(ref target) = save_target {
                        if needs_pfs {
                            mount_pfs(&target.restore_root);
                        }
                        Loading::notify_title("Restoring save...".to_string());
                        match restore_save_target(target, &dl_path) {
                            Ok(_) => Toast::show("Save restored!".to_string()),
                            Err(e) => {
                                error!("restore failed: {:?}", e);
                                Toast::show(format!("Restore failed: {}", e));
                            }
                        }
                    } else {
                        Toast::show("Downloaded (no save target to restore into).".to_string());
                    }
                }
                Err(e) => {
                    error!("download failed: {}", e);
                    Toast::show(format!("Download failed: {}", e));
                }
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    fn delete_from_server(&self) {
        let config = Config::global();
        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }
        if self.cloud_entry.read().unwrap().is_none() {
            Toast::show("No server backup to delete.".to_string());
            return;
        }
        if !UIDialog::present(&format!("Delete server backup for {}?", self.title_name)) {
            return;
        }
        let title_id = self.title_id.clone();
        let cloud_entry = Arc::clone(&self.cloud_entry);
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            match Api::delete_save(&config, &title_id) {
                Ok(_) => {
                    *cloud_entry.write().unwrap() = None;
                    Toast::show("Deleted from server.".to_string());
                }
                Err(e) => {
                    error!("delete {} failed: {}", title_id, e);
                    Toast::show(format!("Delete failed: {}", e));
                }
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    /// Delete the live save data on-device. Native titles only.
    fn delete_game_save(&self, real_id: &str) {
        let real_id = real_id.to_string();
        let name = self.title_name.clone();
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        unmount_pfs();
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

    /// Delete every local backup version for this one game.
    fn delete_selected_game_save(&self) {
        let local_dir = self.local_dir.clone();
        let name = self.title_name.clone();
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
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

    /// Exclude-only: excluded entries drop out of the grid, so this menu
    /// can't be reopened to flip it back. Settings is the way back in.
    fn toggle_sync_exclusion(&mut self) {
        let entry_id = self.entry_id();
        let name = self.title_name.clone();
        let prompt = format!("\"{}\"\nExclude from sync? Undo in Settings.", name);
        if UIDialog::present(&prompt) {
            Config::update(|c| c.set_sync_excluded(&entry_id, true));
            self.sync_exclusion_changed = true;
        }
    }

    fn confirm_thrice(action: &ManageAction) -> bool {
        let mut count = 3;
        loop {
            let prompt = if count == 0 {
                action.to_string()
            } else {
                format!("{}: {}", action, count)
            };
            if !UIDialog::present(&prompt) {
                return false;
            }
            if count == 0 {
                return true;
            }
            count -= 1;
        }
    }
}

/// Selection highlight for one row at `y` (already offset by its index).
fn draw_row_highlight(x: i32, y: i32) {
    vita2d_draw_rect(
        x as f32,
        (y - 21) as f32,
        (SCREEN_WIDTH / 2 - 24) as f32,
        30.0,
        get_active_color(),
    );
    vita2d_draw_rect(
        (x + 2) as f32,
        (y + 2 - 21) as f32,
        (SCREEN_WIDTH / 2 - 28) as f32,
        26.0,
        rgba(0x18, 0x18, 0x18, 0xff),
    );
}

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{} KB", bytes / 1024)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

impl UIList for SaveListManage {
    fn init(&mut self) {
        self.fetch_cloud_entry();
    }

    fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed)
    }

    fn picker_active(&self) -> bool {
        self.folder_picker.is_some()
    }

    fn sync_exclusion_changed(&self) -> bool {
        self.sync_exclusion_changed
    }

    fn take_sync_exclusion_changed(&mut self) -> bool {
        std::mem::take(&mut self.sync_exclusion_changed)
    }

    fn do_backup_game_save(&self, save_target: &Option<SaveTarget>, _input: Option<String>) {
        self.upload_to_server(save_target);
    }

    fn do_delete_game_save(&self, _backup_name: &str) {
        self.delete_from_server();
    }

    fn update(&mut self, save_target: &Option<SaveTarget>, buttons: u32) {
        if self.is_pending() {
            return;
        }

        // Folder picker mode: cross toggles a folder, circle saves and
        // returns to the action list.
        if self.folder_picker.is_some() {
            let mut picker = self.folder_picker.take().unwrap();
            if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
                let excluded: Vec<String> = picker
                    .iter()
                    .filter(|(_, included)| !*included)
                    .map(|(name, _)| name.clone())
                    .collect();
                if let ManageContext::Emulator(entry) = &self.context {
                    let entry_id = entry.id.clone();
                    Config::update(|c| c.set_psp_exclusions(&entry_id, excluded));
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

        let selected_idx = self.list_state.selected_idx;
        if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
            match &self.list[selected_idx as usize] {
                ManageAction::UploadToServer => self.upload_to_server(save_target),
                ManageAction::RestoreFromServer => self.restore_from_server(save_target),
                ManageAction::DeleteFromServer => self.delete_from_server(),
                ManageAction::LaunchApp => {
                    if UIDialog::present(&format!("{}: {}", ManageAction::LaunchApp, self.title_name)) {
                        psv_launch_app_by_title_id(&self.title_id);
                    }
                }
                ManageAction::UpdateAccountId => {
                    if let ManageContext::Native { real_id } = &self.context {
                        if UIDialog::present(&ManageAction::UpdateAccountId) {
                            [
                                format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
                                format!("{}/{}", GAME_SAVE_DIR, real_id),
                            ]
                            .iter()
                            .any(|path| {
                                let sfo_path = format!("{}/sce_sys/param.sfo", path);
                                if Path::new(&sfo_path).exists() {
                                    mount_pfs(path);
                                    if let Ok(()) = update_sfo_file_with_current_account_id(&sfo_path) {
                                        Toast::show("Account ID updated!".to_string());
                                    } else {
                                        Toast::show("Account ID update failed!".to_string());
                                    }
                                    unmount_pfs();
                                    return true;
                                }
                                false
                            });
                        }
                    }
                }
                ManageAction::DeleteGameSave => {
                    if let ManageContext::Native { real_id } = &self.context {
                        let real_id = real_id.clone();
                        if Self::confirm_thrice(&ManageAction::DeleteGameSave) {
                            self.delete_game_save(&real_id);
                        }
                    }
                }
                ManageAction::DeleteSelectedGameSave => {
                    if Self::confirm_thrice(&ManageAction::DeleteSelectedGameSave) {
                        self.delete_selected_game_save();
                    }
                }
                ManageAction::SelectFolders => {
                    if let ManageContext::Emulator(entry) = &self.context {
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
                }
                ManageAction::ToggleSyncExclusion => self.toggle_sync_exclusion(),
            }
        }

        self.list_state.update(self.list.len() as i32, buttons);
    }

    fn draw(&self, left: i32, top: i32) {
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
                let y = top + 68 + 30 * idx;
                if i == selected_idx {
                    draw_row_highlight(x, y);
                }
                let label = format!("{} {}", if *included { "[x]" } else { "[ ]" }, name);
                vita2d_draw_text(x + 8, y, rgba(0xff, 0xff, 0xff, 0xff), 1.0, &label);
            }
            return;
        }

        let size = self.list.len() as i32;
        let ListState {
            top_row,
            selected_idx,
            display_row,
        } = self.list_state;
        let cloud_entry = self.cloud_entry.read().unwrap();
        for idx in 0..display_row {
            let i = top_row + idx;
            if i >= size {
                break;
            }
            let x = left + 12;
            let y = top + 68 + 30 * idx;
            if i == selected_idx {
                draw_row_highlight(x, y);
            }

            let action = &self.list[i as usize];
            let text = match action {
                ManageAction::RestoreFromServer => match cloud_entry.as_ref() {
                    Some(entry) => format!("{} ({})", ManageAction::RestoreFromServer, format_size(entry.size)),
                    None => ManageAction::RestoreFromServer.to_string(),
                },
                _ => action.to_string(),
            };
            let color = match action {
                ManageAction::UploadToServer => rgba(0x00, 0xb4, 0xd8, 0xff),
                ManageAction::RestoreFromServer => rgba(0xff, 0xaa, 0x44, 0xff),
                ManageAction::DeleteFromServer
                | ManageAction::DeleteGameSave
                | ManageAction::DeleteSelectedGameSave => rgba(0xff, 0x88, 0x88, 0xff),
                _ => rgba(0xff, 0xff, 0xff, 0xff),
            };
            vita2d_draw_text(x + 8, y, color, 1.0, &text);
        }

        if cloud_entry.is_none() && !Config::global().is_configured() {
            vita2d_draw_text(
                left + 12,
                top + 68 + 30 * self.list.len() as i32 + 20,
                rgba(0xaa, 0xaa, 0xaa, 0xff),
                1.0,
                "Server not configured.",
            );
        }
    }
}
