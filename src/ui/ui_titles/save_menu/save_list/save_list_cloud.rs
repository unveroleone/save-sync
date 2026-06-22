use std::{
    fs,
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
    constant::SCREEN_WIDTH,
    ime::{get_current_format_time, show_keyboard},
    tai::{mount_pfs, Title},
    ui::{
        ui_cloud::list_state::ListState, ui_dialog::UIDialog, ui_list::UIList, ui_loading::Loading,
        ui_scroll_progress::ScrollProgress, ui_toast::Toast,
    },
    utils::{
        backup_game_save, delete_dir_if_empty, get_active_color, get_game_local_backup_dir,
        restore_game_save, sha256_file,
    },
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, SceCtrlButtons,
    },
};

use super::DISPLAY_ROW;

// Stub kept for compatibility with modules that reference QrCodeState as a type.
pub struct QrCodeState;

impl QrCodeState {
    pub fn new() -> Self {
        QrCodeState
    }
}

enum CloudItem {
    UploadAction,
    ServerBackup(CloudGameEntry),
    DownloadAction,
    DownloadRestoreAction,
}

pub struct SaveListCloud {
    pending: Arc<AtomicBool>,
    list_state: ListState,
    local_dir: String,
    title_id: String,
    title_name: String,
    new_backup_text: &'static str,
    scroll_progress: ScrollProgress,
    cloud_entry: Arc<RwLock<Option<CloudGameEntry>>>,
    config: Arc<RwLock<Config>>,
}

impl SaveListCloud {
    pub fn new(new_backup_text: &'static str, title: &Title, config: Arc<RwLock<Config>>) -> SaveListCloud {
        SaveListCloud {
            list_state: ListState::new(DISPLAY_ROW),
            pending: Arc::new(AtomicBool::new(false)),
            local_dir: get_game_local_backup_dir(&title.title_id(), &title.name()),
            title_id: title.title_id().to_string(),
            title_name: title.name().to_string(),
            new_backup_text,
            scroll_progress: ScrollProgress::new(40.0, 100.0),
            cloud_entry: Arc::new(RwLock::new(None)),
            config,
        }
    }

    fn local_dir(&self) -> String {
        self.local_dir.to_string()
    }

    fn item_count(&self) -> i32 {
        2 + if self.cloud_entry.read().unwrap().is_some() { 2 } else { 0 }
    }

    fn get_cloud_item(&self, idx: i32) -> Option<CloudItem> {
        match idx {
            0 => Some(CloudItem::UploadAction),
            1 => {
                let entry = self.cloud_entry.read().unwrap();
                entry.clone().map(CloudItem::ServerBackup)
            }
            2 => {
                if self.cloud_entry.read().unwrap().is_some() {
                    Some(CloudItem::DownloadAction)
                } else {
                    None
                }
            }
            3 => {
                if self.cloud_entry.read().unwrap().is_some() {
                    Some(CloudItem::DownloadRestoreAction)
                } else {
                    None
                }
            }
            _ => None,
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

    fn upload_current_save(
        &self,
        game_save_dir: &Option<String>,
        input_overwrite: Option<String>,
    ) {
        let game_save_dir = match game_save_dir {
            Some(d) => d.clone(),
            None => {
                Toast::show("No game save found!".to_string());
                return;
            }
        };
        let local_dir = self.local_dir();
        let backup_name = match &input_overwrite {
            Some(input) => format!("{}.zip", input),
            None => {
                let input = show_keyboard(&get_current_format_time());
                if input.is_empty() {
                    Toast::show("Upload cancelled.".to_string());
                    return;
                }
                format!("{}.zip", input)
            }
        };
        let backup_path = format!("{}/{}", local_dir, backup_name);
        let title_id = self.title_id.clone();
        let cloud_entry = Arc::clone(&self.cloud_entry);
        let config = Config::global();

        if !config.is_configured() {
            Toast::show("Configure server in Settings first.".to_string());
            return;
        }

        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();
        mount_pfs(&game_save_dir);
        tokio::spawn(async move {
            Loading::notify_title("Backing up & uploading...".to_string());
            match backup_game_save(&game_save_dir, &backup_path) {
                Ok(_) => {
                    let hash = match sha256_file(&backup_path) {
                        Ok(h) => h,
                        Err(e) => {
                            error!("hash failed: {:?}", e);
                            Toast::show("Hash failed.".to_string());
                            Loading::hide();
                            return;
                        }
                    };
                    let timestamp = get_current_format_time();

                    match Api::upload_save(
                        &config,
                        &title_id,
                        &backup_path,
                        &hash,
                        &timestamp,
                    ) {
                        Ok(_resp) => {
                            // Update local cloud entry
                            if let Ok(manifest) = Api::get_cloud_manifest(&config) {
                                if let Some(entry) = manifest.games.get(&title_id) {
                                    *cloud_entry.write().unwrap() = Some(entry.clone());
                                }
                            }
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
            // Clean up temp backup after upload
            if Path::new(&backup_path).exists() {
                let _ = fs::remove_file(&backup_path);
                let _ = delete_dir_if_empty(&local_dir);
            }
            Loading::hide();
            pending.store(false, Ordering::Relaxed);
        });
    }

    fn download_from_server(&self, game_save_dir: &Option<String>, restore: bool) {
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

        let action = if restore { "Restore from server" } else { "Download from server" };
        if !UIDialog::present(&format!(
            "{}?\n{} ({})",
            action,
            entry.latest_version,
            format_size(entry.size)
        )) {
            return;
        }

        let title_id = self.title_id.clone();
        let local_dir = self.local_dir();
        let game_save_dir = game_save_dir.clone();
        let pending = Arc::clone(&self.pending);
        pending.store(true, Ordering::Relaxed);
        Loading::show();

        tokio::spawn(async move {
            Loading::notify_title(if restore {
                "Downloading & restoring..."
            } else {
                "Downloading from server..."
            }.to_string());

            let dl_name = format!("{}.zip", get_current_format_time());
            let dl_path = format!("{}/{}", local_dir, dl_name);

            match Api::download_save(&config, &title_id, &dl_path) {
                Ok(_) => {
                    if restore {
                        if let Some(ref gsd) = game_save_dir {
                            mount_pfs(gsd);
                            Loading::notify_title("Restoring save...".to_string());
                            match restore_game_save(&dl_path, gsd) {
                                Ok(_) => Toast::show("Save restored!".to_string()),
                                Err(e) => {
                                    error!("restore failed: {:?}", e);
                                    Toast::show(format!("Restore failed: {}", e));
                                }
                            }
                        }
                    } else {
                        Toast::show("Download complete!".to_string());
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

impl UIList for SaveListCloud {
    fn init(&mut self) {
        self.fetch_cloud_entry();
    }

    fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Relaxed)
    }

    fn do_backup_game_save(&self, game_save_dir: &Option<String>, _input: Option<String>) {
        self.upload_current_save(game_save_dir, None);
    }

    fn do_delete_game_save(&self, _backup_name: &str) {}

    fn update(&mut self, game_save_dir: &Option<String>, buttons: u32) {
        self.scroll_progress.update(buttons);
        let selected_idx = self.list_state.selected_idx;

        if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
            if let Some(item) = self.get_cloud_item(selected_idx) {
                match item {
                    CloudItem::UploadAction => {
                        self.do_backup_game_save(game_save_dir, None);
                    }
                    CloudItem::DownloadAction => {
                        self.download_from_server(game_save_dir, false);
                    }
                    CloudItem::DownloadRestoreAction => {
                        self.download_from_server(game_save_dir, true);
                    }
                    CloudItem::ServerBackup(_) => {
                        // Selecting the server backup info: download
                        self.download_from_server(game_save_dir, false);
                    }
                }
            }
        }

        self.list_state.update(self.item_count(), buttons);
    }

    fn draw(&self, left: i32, top: i32) {
        let size = self.item_count();
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
            let y = top + 68;
            let h = 30 * idx;

            if i == selected_idx {
                vita2d_draw_rect(
                    x as f32,
                    (y + h - 21) as f32,
                    (SCREEN_WIDTH / 2 - 24) as f32,
                    30.0,
                    get_active_color(),
                );
                vita2d_draw_rect(
                    (x + 2) as f32,
                    (y + 2 + h - 21) as f32,
                    (SCREEN_WIDTH / 2 - 28) as f32,
                    26.0,
                    rgba(0x18, 0x18, 0x18, 0xff),
                );
            }

            let text = match self.get_cloud_item(i) {
                Some(CloudItem::UploadAction) => self.new_backup_text.to_string(),
                Some(CloudItem::ServerBackup(ref entry)) => {
                    format!("Server: {} ({})", entry.latest_version, format_size(entry.size))
                }
                Some(CloudItem::DownloadAction) => "Download from server".to_string(),
                Some(CloudItem::DownloadRestoreAction) => "Download & Restore".to_string(),
                None => continue,
            };

            let color = match self.get_cloud_item(i) {
                Some(CloudItem::UploadAction) => rgba(0x00, 0xb4, 0xd8, 0xff),
                Some(CloudItem::ServerBackup(_)) => rgba(0xee, 0xee, 0xee, 0xff),
                Some(CloudItem::DownloadAction) => rgba(0x88, 0xff, 0x88, 0xff),
                Some(CloudItem::DownloadRestoreAction) => rgba(0xff, 0xaa, 0x44, 0xff),
                None => rgba(0xff, 0xff, 0xff, 0xff),
            };

            vita2d_draw_text(x + 8, y + h, color, 1.0, &text);
        }

        // Show "not configured" if needed
        if self.cloud_entry.read().unwrap().is_none() && !Config::global().is_configured() {
            let msg = "Server not configured.";
            vita2d_draw_text(
                left + 12,
                350,
                rgba(0xaa, 0xaa, 0xaa, 0xff),
                1.0,
                msg,
            );
        }
    }
}
