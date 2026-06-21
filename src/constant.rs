use std::time::Duration;

pub const SCREEN_WIDTH: i32 = 960;
pub const SCREEN_HEIGHT: i32 = 544;

// psv game save paths
pub const GAME_CARD_SAVE_DIR: &str = "grw0:savedata";
pub const GAME_SAVE_DIR: &str = "ux0:user/00/savedata";
pub const PSV_DEVICES: [&str; 11] = [
    "ux0:", "uma0:", "grw0:", "os0:", "pd0:", "sa0:", "tm0:", "ud0:", "ur0:", "vd0:", "vs0:",
];

// app data root (replaces ux0:data/save-cloud)
pub const SAVE_CLOUD_DIR: &str = "ux0:data/save-sync";
// local backup archive directory (replaces ux0:data/save-cloud/saves)
pub const GAME_SAVE_LOCAL_DIR: &str = "ux0:data/save-sync/backups";
// config and manifest
pub const CONFIG_PATH: &str = "ux0:data/save-sync/config.json";
pub const LOCAL_MANIFEST_PATH: &str = "ux0:data/save-sync/manifest.json";
// staging area for downloaded saves before restore
pub const DOWNLOADS_DIR: &str = "ux0:data/save-sync/downloads";
// log
pub const SAVE_LOG_PATH: &str = "ux0:data/save-sync/log/log.txt";

// ssl certificates (needed for HTTPS to self-hosted server)
pub const SSL_CERT_ENV_KEY: &str = "SSL_CERT_FILE";
pub const SAVE_SYNC_CERT: &str = "ux0:data/save-sync/cacert.pem";
pub const PSV_DEVICE_CERT: &str = "vs0:data/external/cert/CA_LIST.cer";

// button input timing
pub const BUTTON_HOLDING_DELAY: u128 = 360;
pub const BUTTON_HOLDING_REPEAT_DELAY: u128 = 60;

// desktop / nav labels
pub const TEXT_L: &str = "L <-";
pub const TEXT_R: &str = "-> R";

// desktop bottom bar
pub const DESKTOP_BOTTOM_BAR_TEXT: &str =
    "(START) Exit    (□) About    (△) Menu    (X) Saves";
pub const DESKTOP_BOTTOM_BAR_CLOUD_TEXT: &str =
    "(START) Exit";

// save drawer (local tab)
pub const SAVE_DRAWER_BOTTOM_BAR_TEXT: &str =
    "(SEL)Up  (□)Restore  (△)Del  (〇)Close  (X)Sel";
// save drawer (cloud/server tab)
pub const SAVE_DRAWER_CLOUD_BOTTOM_BAR_TEXT: &str =
    "(SEL) Download  (□) Restore  (△) Delete  (〇) Close  (X) Select";
pub const ACTION_DRAWER_BOTTOM_BAR_TEXT: &str = "(〇) Close    (X) Select";
pub const TITLE_DRAWER_BOTTOM_BAR_TEXT: &str = "(〇) Close    (X) Select";

// save menu tabs
pub const TAB_LOCAL: &str = "Local Backup";
pub const TAB_CLOUD: &str = "Server Backup";
pub const NEW_BACKUP: &str = "New Backup";
pub const NEW_CLOUD_BACKUP: &str = "Upload to Server";

// blacklist: files never included in a save backup zip
pub const BACKUP_BLACK_LIST: [&str; 4] = [
    "sce_pfs",
    "sce_sys/safemem.dat",
    "sce_sys/keystone",
    "sce_sys/sealedkey",
];

// animation durations
pub const ANIME_TIME_300: Duration = Duration::from_millis(300);
pub const ANIME_TIME_160: Duration = Duration::from_millis(160);

// upload / download buffer sizes
pub const UPLOAD_SLICE_PER_SIZE: usize = 1024 * 1024 * 4; // 4 MiB
pub const DOWNLOAD_BUF_SIZE: usize = 1024 * 512; // 512 KiB

// list display
pub const LIST_NAME_WIDTH: i32 = SCREEN_WIDTH / 2 - 40;

// dialog dimensions
pub const DIALOG_WIDTH: i32 = 600;
pub const DIALOG_HEIGHT: i32 = 260;
pub const DIALOG_BOTTOM_TOP: i32 = 220;
pub const DIALOG_CONFIRM_TEXT: &str = "(X) Confirm";
pub const DIALOG_CANCEL_TEXT: &str = "(〇) Cancel";

// about dialog text
pub const ABOUT_TEXT: &str = "Save Sync v0.1.0 — Two-Vita save sync tool";
