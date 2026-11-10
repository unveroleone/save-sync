use std::{
    fs,
    sync::{Arc, RwLock},
};

use crate::{
    app::AppData,
    config::Config,
    constant::{
        DESKTOP_BOTTOM_BAR_SETTINGS_TEXT, DESKTOP_BOTTOM_BAR_TEXT, SCREEN_HEIGHT, SCREEN_WIDTH,
        TEXT_L, TEXT_R,
    },
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_draw_texture, vita2d_line,
        vita2d_load_jpg_buf, vita2d_load_png_buf, vita2d_text_height, vita2d_text_width,
        SceCtrlButtons, Vita2dTexture,
    },
};

use super::{ui_base::UIBase, ui_settings::UISettings};

const IMAGE_ICON_PATH: &str = "app0:sce_sys/resources/icon.png";
const IMAGE_DEVICE_PATH: &str = "app0:sce_sys/resources/device.png";
const IMAGE_SETTINGS_PATH: &str = "app0:sce_sys/resources/settings.png";
const IMAGE_DEVICE_BG_PATH: &str = "app0:sce_sys/resources/device_bg.jpg";
const ICON_SIZE: i32 = 70;
const ICON_OFFSET: i32 = 10;
const ICON_GAP: i32 = 20;
const TEXT_VERSION: &str = concat!("v", env!("CARGO_PKG_VERSION"));
const TEXT_CREDIT: &str = "by unveroleone";

pub struct UIDesktop {
    selected_idx: i32,
    pub games: Box<dyn UIBase>,
    settings: Option<UISettings>,
    pub assets: Vec<Vita2dTexture>,
    asset_bufs: Arc<RwLock<Option<Vec<Vec<u8>>>>>,
}

impl UIDesktop {
    pub fn new(games: Box<dyn UIBase>) -> UIDesktop {
        let res = UIDesktop {
            selected_idx: 0,
            games,
            settings: None,
            assets: vec![],
            asset_bufs: Arc::new(RwLock::new(None)),
        };
        res.init_assets();
        res
    }

    // async load images
    fn init_assets(&self) {
        let asset_bufs = Arc::clone(&self.asset_bufs);
        tokio::spawn(async move {
            for path in [
                IMAGE_DEVICE_BG_PATH,
                IMAGE_ICON_PATH,
                IMAGE_DEVICE_PATH,
                IMAGE_SETTINGS_PATH,
            ]
            .into_iter()
            {
                if let Ok(file) = fs::read(path) {
                    let mut lock = asset_bufs.write().expect("get asset_bufs write lock");
                    if lock.is_none() {
                        *lock = Some(vec![]);
                    }
                    if let Some(ref mut list) = *lock {
                        list.push(file);
                    }
                }
                tokio::task::yield_now().await;
            }
        });
    }

    // init textures
    fn init_assets_textures(&mut self) {
        if self.assets.len() >= 4 {
            return;
        }
        if let Ok(lock) = self.asset_bufs.try_read() {
            if let Some(ref asset_bufs) = *lock {
                for (idx, buf) in asset_bufs.into_iter().enumerate() {
                    if idx >= self.assets.len() {
                        if idx == 0 {
                            self.assets.push(vita2d_load_jpg_buf(buf));
                        } else {
                            self.assets.push(vita2d_load_png_buf(buf));
                        }
                    }
                }
            }
            drop(lock);
        }

        if self.assets.len() == 4 {
            let mut lock = self.asset_bufs.write().expect("get asset_bufs write lock");
            *lock = None;
            drop(lock);
        }
    }

    fn draw_top_line(&self) {
        // version
        vita2d_draw_text(
            ICON_OFFSET + (70 - vita2d_text_width(0.61, TEXT_VERSION)) / 2,
            70 + vita2d_text_height(0.61, TEXT_VERSION) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            0.61,
            TEXT_VERSION,
        );
        // credit
        vita2d_draw_text(
            ICON_OFFSET + (70 - vita2d_text_width(0.51, TEXT_CREDIT)) / 2,
            82 + vita2d_text_height(0.51, TEXT_CREDIT) / 2,
            rgba(0xaa, 0xaa, 0xaa, 0xff),
            0.51,
            TEXT_CREDIT,
        );
        vita2d_draw_text(
            ICON_OFFSET + ICON_SIZE + ICON_GAP - vita2d_text_width(0.61, TEXT_L),
            80 + vita2d_text_height(0.61, TEXT_L) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            0.61,
            TEXT_L,
        );
        vita2d_draw_text(
            ICON_OFFSET + (ICON_SIZE + ICON_GAP) * 3 - ICON_GAP,
            80 + vita2d_text_height(0.61, TEXT_R) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            0.61,
            TEXT_R,
        );
        // top line
        vita2d_line(0.0, 90.0, 960.0, 90.0, rgba(0x99, 0x99, 0x99, 0xff));
    }

    fn draw_icon(&self) {
        if self.assets.len() > 1 {
            vita2d_draw_texture(
                &self.assets[1],
                ICON_OFFSET as f32,
                ICON_OFFSET as f32 - 2.0,
            );
        }
    }

    fn draw_device(&self) {
        if self.assets.len() > 2 {
            vita2d_draw_texture(
                &self.assets[2],
                (ICON_OFFSET + ICON_SIZE * 1 + ICON_GAP) as f32,
                ICON_OFFSET as f32,
            );
        }
    }

    fn draw_settings_icon(&self) {
        if self.assets.len() > 3 {
            vita2d_draw_texture(
                &self.assets[3],
                (ICON_OFFSET + ICON_SIZE * 2 + ICON_GAP * 2) as f32,
                ICON_OFFSET as f32,
            );
        }
    }

    fn draw_device_bg(&self) {
        if self.assets.len() > 0 {
            vita2d_draw_texture(&self.assets[0], 0.0, 0.0);
        }
    }

    fn draw_selected_rect(&self) {
        vita2d_draw_rect(
            if self.selected_idx == 0 {
                (ICON_OFFSET + ICON_SIZE + ICON_GAP - 5) as f32
            } else {
                (ICON_OFFSET + (ICON_SIZE + ICON_GAP) * 2 - 5) as f32
            },
            5.0,
            80.0,
            80.0,
            rgba(0x66, 0x66, 0x66, 0xff),
        );
    }

    fn settings_forces(&self) -> bool {
        self.settings.as_ref().map(|s| s.is_forces()).unwrap_or(false)
    }

    /// Drop the settings screen, return to the Games tab, and pick up
    /// whatever changed while it was open (e.g. sync exclusions).
    fn leave_settings(&mut self) {
        self.settings = None;
        self.selected_idx = 0;
        self.games.invalidate();
    }

    fn draw_bottom_bar(&self) {
        vita2d_line(
            0.0,
            (SCREEN_HEIGHT - 58) as f32,
            SCREEN_WIDTH as f32,
            (SCREEN_HEIGHT - 58) as f32,
            rgba(0x99, 0x99, 0x99, 0xff),
        );
        let bottom_bar_text = if self.selected_idx == 0 {
            DESKTOP_BOTTOM_BAR_TEXT
        } else {
            DESKTOP_BOTTOM_BAR_SETTINGS_TEXT
        };
        vita2d_draw_text(
            SCREEN_WIDTH - 12 - vita2d_text_width(1.0, bottom_bar_text),
            SCREEN_HEIGHT - (58 / 2) + vita2d_text_height(1.0, bottom_bar_text) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            bottom_bar_text,
        )
    }
}

impl UIBase for UIDesktop {
    fn is_forces(&self) -> bool {
        self.games.is_forces() || self.settings_forces()
    }

    fn update(&mut self, app_data: &mut AppData, buttons: u32) {
        // update textures
        UIDesktop::init_assets_textures(self);

        // Entering the settings tab: construct fresh from the current
        // config so it never shows stale state from a previous visit (and
        // never clobbers a change made elsewhere, e.g. an exclusion toggled
        // from the Games tab's own menu while Settings was last open).
        if self.selected_idx == 1 && self.settings.is_none() {
            self.settings = Some(UISettings::new(&Config::global()));
        }

        let forces = if self.selected_idx == 0 {
            self.games.update(app_data, buttons);
            self.games.is_forces()
        } else if let Some(settings) = &mut self.settings {
            settings.update(app_data, buttons);
            if settings.should_close {
                self.leave_settings();
                false
            } else {
                settings.is_forces()
            }
        } else {
            false
        };

        if forces {
            return;
        }
        let both_held = is_button(buttons, SceCtrlButtons::SceCtrlLtrigger)
            && is_button(buttons, SceCtrlButtons::SceCtrlRtrigger);
        if both_held {
            return;
        }
        if self.selected_idx == 1 && is_button(buttons, SceCtrlButtons::SceCtrlLtrigger) {
            self.leave_settings();
        } else if self.selected_idx == 0 && is_button(buttons, SceCtrlButtons::SceCtrlRtrigger) {
            self.selected_idx = 1;
        }
    }

    fn draw(&self, app_data: &AppData) {
        // draw device_bg
        self.draw_device_bg();
        if self.assets.len() > 2 {
            // draw active bg
            self.draw_selected_rect();
        }
        // draw top line
        self.draw_top_line();
        // draw icon
        self.draw_icon();
        // draw device
        self.draw_device();
        // draw settings tab icon
        self.draw_settings_icon();

        // draw selected child (draw first so bottom bar renders on top)
        let child_forces = if self.selected_idx == 0 {
            self.games.draw(app_data);
            self.games.is_forces()
        } else if let Some(settings) = &self.settings {
            settings.draw(app_data);
            settings.is_forces()
        } else {
            false
        };

        if !child_forces {
            self.draw_bottom_bar();
        }
    }
}
