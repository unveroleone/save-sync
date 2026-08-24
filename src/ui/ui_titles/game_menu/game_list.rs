use std::{
    ffi::OsStr,
    fmt::{Display, Formatter},
    fs,
    ops::Deref,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use log::error;

use crate::{
    config::Config,
    constant::{GAME_CARD_SAVE_DIR, GAME_SAVE_DIR, SCREEN_WIDTH},
    emulator::{EmulatorEntry, EmulatorKind},
    tai::{mount_pfs, psv_launch_app_by_title_id, unmount_pfs, Title, Titles},
    ui::{
        list_state::ListState, ui_dialog::UIDialog, ui_loading::Loading, ui_toast::Toast,
    },
    utils::{get_active_color, get_game_local_backup_dir, update_sfo_file_with_current_account_id},
    vita2d::{is_button, rgba, vita2d_draw_rect, vita2d_draw_text, SceCtrlButtons},
};

enum GameMenuAction {
    LaunchApp,
    UpdateAccountId,
    DeleteGameSave,
    DeleteSelectedGameSave,
    DeleteAllGameSaves,
    SelectFolders,
    ToggleSyncExclusion,
}

impl Deref for GameMenuAction {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            GameMenuAction::LaunchApp => "Launch Game",
            GameMenuAction::UpdateAccountId => "Update Account ID",
            GameMenuAction::DeleteGameSave => "Delete Game Save",
            GameMenuAction::DeleteSelectedGameSave => "Delete Local Backup",
            GameMenuAction::DeleteAllGameSaves => "Delete All Local Backups",
            GameMenuAction::SelectFolders => "Select Folders",
            GameMenuAction::ToggleSyncExclusion => "Exclude from Sync",
        }
    }
}

impl Display for GameMenuAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.deref())
    }
}

/// Whose save the menu was opened for. Native titles get the full action set,
/// while emulator entries (PSP/RetroArch) get the subset that makes sense for
/// a single entry: local-backup deletion, and folder selection for PSP.
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
    sync_exclusion_changed: bool,
}

impl GameList {
    pub fn new() -> Self {
        let mut list = GameList {
            pending: Arc::new(AtomicBool::new(false)),
            list_state: ListState::new(15),
            list: Vec::new(),
            mode: GameListMode::Native,
            folder_picker: None,
            sync_exclusion_changed: false,
        };
        list.set_native();
        list
    }

    /// Native action set.
    pub fn set_native(&mut self) {
        self.mode = GameListMode::Native;
        self.list = vec![
            GameMenuAction::LaunchApp,
            GameMenuAction::UpdateAccountId,
            GameMenuAction::DeleteGameSave,
            GameMenuAction::DeleteSelectedGameSave,
            GameMenuAction::DeleteAllGameSaves,
            GameMenuAction::ToggleSyncExclusion,
        ];
        self.folder_picker = None;
        self.list_state.reset();
    }

    /// Emulator action set. LaunchApp and UpdateAccountId are native-only, and
    /// the whole-device delete operations would surprise from a single PSP or
    /// RetroArch entry.
    pub fn set_emulator(&mut self, entry: &EmulatorEntry) {
        self.mode = GameListMode::Emulator(entry.clone());
        let mut actions = vec![GameMenuAction::DeleteSelectedGameSave];
        // The folder picker only makes sense when a PSP game owns several
        // folders (save slots + DLC/install data).
        if entry.kind == EmulatorKind::Psp && entry.all_paths().len() > 1 {
            actions.push(GameMenuAction::SelectFolders);
        }
        actions.push(GameMenuAction::ToggleSyncExclusion);
        self.list = actions;
        self.folder_picker = None;
        self.list_state.reset();
    }

    pub fn picker_active(&self) -> bool {
        self.folder_picker.is_some()
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

    /// Exclude-only: excluded entries drop out of the grid, so this menu
    /// can't be reopened to flip it back. Settings is the way back in.
    fn toggle_sync_exclusion(&mut self, entry_id: &str, name: &str) {
        let prompt = format!("\"{}\"\nExclude from sync? Undo in Settings.", name);
        if UIDialog::present(&prompt) {
            Config::update(|c| c.set_sync_excluded(entry_id, true));
            self.sync_exclusion_changed = true;
        }
    }

    pub fn sync_exclusion_changed(&self) -> bool {
        self.sync_exclusion_changed
    }

    /// True (once) if an exclusion changed since the last check.
    pub fn take_sync_exclusion_changed(&mut self) -> bool {
        std::mem::take(&mut self.sync_exclusion_changed)
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

    pub fn update(
        &mut self,
        buttons: u32,
        title: Option<&Title>,
        titles: &Titles,
        emu: Option<&EmulatorEntry>,
    ) {
        if self.is_pending() {
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
                        GameMenuAction::ToggleSyncExclusion => {
                            let id = entry.id.clone();
                            let name = entry.name.clone();
                            self.toggle_sync_exclusion(&id, &name);
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
                GameMenuAction::ToggleSyncExclusion => {
                    self.toggle_sync_exclusion(title.title_id(), title.name());
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
