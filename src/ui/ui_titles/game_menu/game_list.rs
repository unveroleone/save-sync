use std::{
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
    constant::{GAME_CARD_SAVE_DIR, GAME_SAVE_DIR, SCREEN_WIDTH},
    ime::get_current_format_time,
    tai::{mount_pfs, psv_launch_app_by_title_id, unmount_pfs, Title, Titles},
    ui::{
        ui_cloud::list_state::ListState, ui_dialog::UIDialog, ui_loading::Loading, ui_toast::Toast,
    },
    utils::{
        backup_game_save, get_active_color, get_game_local_backup_dir,
        update_sfo_file_with_current_account_id,
    },
    vita2d::{is_button, rgba, vita2d_draw_rect, vita2d_draw_text, SceCtrlButtons},
};

enum GameMenuAction {
    LaunchApp,
    BackupAllGameSave,
    UpdateAccountId,
    DeleteGameSave,
    DeleteSelectedGameSave,
    DeleteAllGameSaves,
}

impl Deref for GameMenuAction {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        match self {
            GameMenuAction::LaunchApp => "Launch Game",
            GameMenuAction::BackupAllGameSave => "Backup All Game Saves",
            GameMenuAction::UpdateAccountId => "Update Account ID",
            GameMenuAction::DeleteGameSave => "Delete Game Save",
            GameMenuAction::DeleteSelectedGameSave => "Delete Local Backup",
            GameMenuAction::DeleteAllGameSaves => "Delete All Local Backups",
        }
    }
}

impl Display for GameMenuAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.deref())
    }
}

pub struct GameList {
    pending: Arc<AtomicBool>,
    list_state: ListState,
    list: [GameMenuAction; 6],
    game_save_dir_prepare_to_mount: Arc<RwLock<Option<String>>>,
    game_save_dir_on_mounted: Arc<RwLock<Option<String>>>,
}

impl GameList {
    pub fn new() -> Self {
        GameList {
            pending: Arc::new(AtomicBool::new(false)),
            list_state: ListState::new(15),
            list: [
                GameMenuAction::LaunchApp,
                GameMenuAction::BackupAllGameSave,
                GameMenuAction::UpdateAccountId,
                GameMenuAction::DeleteGameSave,
                GameMenuAction::DeleteSelectedGameSave,
                GameMenuAction::DeleteAllGameSaves,
            ],
            game_save_dir_prepare_to_mount: Arc::new(RwLock::new(None)),
            game_save_dir_on_mounted: Arc::new(RwLock::new(None)),
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

        let game_save_dir_on_mounted = Arc::clone(&self.game_save_dir_on_mounted);
        let game_save_dir_prepare_to_mount = Arc::clone(&self.game_save_dir_prepare_to_mount);
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        tokio::spawn(async move {
            let mut backup_failed_count = 0;
            for (idx, (title_id, real_id, name)) in list.iter().enumerate() {
                Loading::notify_title(format!(
                    "Backing up ({}/{}): {}",
                    idx + 1,
                    list.len(),
                    name
                ));
                let dirs = [
                    format!("{}/{}", GAME_CARD_SAVE_DIR, real_id),
                    format!("{}/{}", GAME_SAVE_DIR, real_id),
                ];
                let game_save_dir = dirs.iter().find(|dir| Path::new(&dir).exists());
                if game_save_dir.is_none() {
                    continue;
                }
                let game_save_dir = game_save_dir.unwrap();
                let mut is_prepare = false;
                loop {
                    if let Ok(game_save_dir_on_mounted) = game_save_dir_on_mounted.try_read() {
                        if let Some(game_save_dir_on_mounted) = game_save_dir_on_mounted.as_ref() {
                            if game_save_dir_on_mounted == game_save_dir {
                                break;
                            }
                        }
                    }
                    if !is_prepare {
                        if let Ok(mut game_save_dir_prepare_to_mount) =
                            game_save_dir_prepare_to_mount.try_write()
                        {
                            is_prepare = true;
                            *game_save_dir_prepare_to_mount = Some(game_save_dir.clone());
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                let backup_to_path = format!(
                    "{}/{}.zip",
                    get_game_local_backup_dir(&title_id, &name),
                    get_current_format_time()
                );
                match backup_game_save(game_save_dir, &backup_to_path) {
                    Err(err) => {
                        backup_failed_count += 1;
                        error!(
                            "zip {} to {} failed: {:?}",
                            game_save_dir, backup_to_path, err
                        );
                        Toast::show(format!("Backup failed for {}!", name));
                    }
                    _ => {}
                }
            }
            if backup_failed_count == 0 {
                Toast::show("All backups complete!".to_string());
            } else {
                Toast::show(format!("{} backups failed!", backup_failed_count));
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
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

    pub fn update(&mut self, buttons: u32, title: &Title, titles: &Titles) {
        self.mount_game_dir_if_exists();

        if self.is_pending() {
            return;
        }
        let ListState { selected_idx, .. } = self.list_state;
        if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
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
            }
        }

        self.list_state.update(self.list.len() as i32, buttons);
    }

    pub fn draw(&self, left: i32, top: i32) {
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
