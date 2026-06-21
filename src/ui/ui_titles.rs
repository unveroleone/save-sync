use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{Arc, RwLock},
};

use log::error;

use crate::{
    app::AppData,
    constant::{ABOUT_TEXT, GAME_CARD_SAVE_DIR, GAME_SAVE_DIR},
    utils::get_active_color,
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_draw_texture_scale,
        vita2d_load_png_buf, vita2d_text_height, SceCtrlButtons, Vita2dTexture,
    },
};

use self::{game_menu::GameMenu, save_menu::SaveMenu};

use super::{ui_base::UIBase, ui_dialog::UIDialog};

pub mod game_menu;
pub mod save_menu;

const ICON_SIZE: i32 = 94;
const ICON_COL: i32 = 10;
const ICON_ROW: i32 = 4;
const OFFSET_TOP: i32 = 100;
const OFFSET_LEFT: i32 = 10;

pub struct UITitles {
    pub top_row: i32,
    pub selected_idx: i32,
    pub icons: HashMap<u32, Vita2dTexture>,
    pub icon_bufs: Arc<RwLock<HashMap<u32, Option<Vec<u8>>>>>,
    save_menu: SaveMenu,
    game_menu: GameMenu,
}

impl UITitles {
    pub fn new() -> UITitles {
        UITitles {
            top_row: 0,
            selected_idx: 0,
            icons: HashMap::new(),
            icon_bufs: Arc::new(RwLock::new(HashMap::new())),
            save_menu: SaveMenu::new(),
            game_menu: GameMenu::new(),
        }
    }

    fn update_selected(&mut self, app_data: &mut AppData, buttons: u32) {
        let size = app_data.titles.size() as i32;
        let idx = self.selected_idx;
        let top = self.top_row;
        match buttons {
            _ if is_button(buttons, SceCtrlButtons::SceCtrlLeft) => {
                if idx > 0 {
                    self.selected_idx = idx - 1;
                }
                if self.selected_idx < ICON_COL * top && top > 0 {
                    self.top_row -= 1;
                }
            }
            _ if is_button(buttons, SceCtrlButtons::SceCtrlRight) => {
                if idx < size - 1 {
                    self.selected_idx += 1;
                }
                if self.selected_idx - top * ICON_COL >= ICON_COL * ICON_ROW {
                    self.top_row += 1;
                }
            }
            _ if is_button(buttons, SceCtrlButtons::SceCtrlUp) => {
                if idx / ICON_COL == 0 {
                    let rows = (size - 1) / ICON_COL + 1;
                    self.selected_idx = self.selected_idx % ICON_COL + (rows - 1) * ICON_COL;
                    if self.selected_idx >= size {
                        self.selected_idx = size - 1;
                    }
                    self.top_row = if rows >= ICON_ROW { rows - ICON_ROW } else { 0 };
                } else if self.selected_idx >= ICON_COL {
                    self.selected_idx = self.selected_idx - ICON_COL;
                    // scroll down
                    if self.selected_idx < ICON_COL * self.top_row {
                        self.top_row -= 1;
                    }
                }
            }
            _ if is_button(buttons, SceCtrlButtons::SceCtrlDown) => {
                if (idx + ICON_COL) / ICON_COL > (size - 1) / ICON_COL {
                    self.selected_idx = self.selected_idx % ICON_COL;
                    self.top_row = 0;
                } else {
                    if idx + ICON_COL < size {
                        self.selected_idx = self.selected_idx + ICON_COL;
                        // scroll up
                        if self.selected_idx - self.top_row * ICON_COL >= ICON_COL * ICON_ROW {
                            self.top_row += 1;
                        }
                    } else if idx % ICON_COL > (size - 1) % ICON_COL {
                        self.selected_idx = size - 1;
                        // scroll up
                        if self.selected_idx - self.top_row * ICON_COL >= ICON_COL * ICON_ROW {
                            self.top_row += 1;
                        }
                    }
                }
            }
            _ => {}
        };
    }

    fn update_icons(&mut self, app_data: &mut AppData) {
        // check icons
        let size = app_data.titles.size() as i32;
        let start_idx = (self.top_row - 1) * ICON_COL;
        let start_idx = if start_idx < 0 { 0 } else { start_idx };
        let end_idx = start_idx + ICON_COL * (ICON_ROW + 2);
        let end_idx = if end_idx < size { end_idx } else { size };

        for (idx, title) in app_data.titles.iter().enumerate() {
            if idx >= start_idx as usize && idx < end_idx as usize {
                let has_icon = self.icons.contains_key(&(idx as u32));
                if has_icon {
                    continue;
                }

                // check icon buf
                if let Ok(mut icon_bufs) = self.icon_bufs.try_write() {
                    if icon_bufs.contains_key(&(idx as u32)) {
                        if let Some(buf) = icon_bufs.get(&(idx as u32)).expect("get icon bufs") {
                            self.icons
                                .insert(idx as u32, vita2d_load_png_buf(buf.as_slice()));
                            icon_bufs.remove(&(idx as u32));
                        }
                        drop(icon_bufs);
                        continue;
                    }
                    // insert None
                    icon_bufs.insert(idx as u32, None);
                    drop(icon_bufs);

                    // load icon buf
                    let iconpath = title.iconpath().to_string();
                    let icon_bufs = Arc::clone(&self.icon_bufs);
                    tokio::spawn(async move {
                        if Path::new(&iconpath).exists() {
                            let file = fs::read(iconpath).expect("open icon file");
                            icon_bufs
                                .write()
                                .expect("get write lock of icon bufs in spawn")
                                .insert(idx as u32, Some(file));
                        } else {
                            error!("app iconpath not exists: {}", iconpath);
                        }
                    });
                }
            } else {
                if self.icons.contains_key(&(idx as u32)) {
                    self.icons.remove(&(idx as u32));
                }
            }
        }
    }

    fn draw_selected_game_info(&self, app_data: &AppData) {
        let titles = &app_data.titles;
        if titles.size() == 0 {
            return;
        }
        let title = titles
            .get_title_by_idx(self.selected_idx)
            .expect("get title id by idx");
        let title_id = title.title_id();
        let real_id = title.real_id();
        let title = format!(
            "{}  |  {}",
            title_id,
            titles
                .get_title_by_idx(self.selected_idx)
                .expect("get title by idx")
                .name(),
        );
        let mut save_path = format!("{}/{}", GAME_CARD_SAVE_DIR, real_id);
        if !Path::new(&save_path).exists() {
            save_path = format!("{}/{}", GAME_SAVE_DIR, real_id);
        }
        let num = format!("→ {}/{}", self.selected_idx + 1, titles.size());

        let left = 330;
        // title
        vita2d_draw_text(
            left,
            10 + vita2d_text_height(1.0, &title),
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            &title,
        );
        // save path

        vita2d_draw_text(
            left,
            35 + vita2d_text_height(1.0, &save_path),
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            if Path::new(&save_path).exists() {
                &save_path
            } else {
                "No saves found"
            },
        );
        // num
        vita2d_draw_text(
            left,
            60 + vita2d_text_height(1.0, &num),
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            &num,
        );

        // selected icon bg
        vita2d_draw_rect(
            (10 + (self.selected_idx % ICON_COL) * ICON_SIZE - 3) as f32,
            100.0 + (((self.selected_idx - self.top_row * ICON_COL) / ICON_COL) * ICON_SIZE) as f32
                - 3.0,
            100.0,
            100.0,
            get_active_color(),
        );
    }

    pub fn draw_game_list(&self, app_data: &AppData) {
        // icon bg
        let icon_bg = rgba(0x44, 0x44, 0x44, 0xff);
        let start_idx = self.top_row * ICON_COL;
        let end_idx = start_idx + ICON_COL * ICON_ROW;
        let size = app_data.titles.size() as i32;
        let end_idx = if end_idx < size { end_idx } else { size };

        for idx in 0..((ICON_COL * ICON_ROW) as i32) {
            if start_idx + idx >= end_idx as i32 {
                continue;
            }
            let icon_idx = (start_idx as i32 + idx) as u32;
            let pad = if icon_idx as i32 == self.selected_idx {
                0
            } else {
                8
            };
            let x = (idx % ICON_COL) * ICON_SIZE + (pad / 2) + OFFSET_LEFT;
            let y = (idx / ICON_COL) * ICON_SIZE + (pad / 2) + OFFSET_TOP;
            vita2d_draw_rect(
                x as f32,
                y as f32,
                (ICON_SIZE - pad) as f32,
                (ICON_SIZE - pad) as f32,
                icon_bg,
            );
            if self.icons.contains_key(&icon_idx) {
                vita2d_draw_texture_scale(
                    self.icons.get(&icon_idx).expect("get icon texture"),
                    x as f32,
                    y as f32,
                    (ICON_SIZE - pad) as f32 / 128.0,
                    (ICON_SIZE - pad) as f32 / 128.0,
                )
            }
        }
    }

    pub fn draw_menu(&self) {
        if self.save_menu.is_active() {
            self.save_menu.draw();
        }

        if self.game_menu.is_active() {
            self.game_menu.draw();
        }
    }
}

impl UIBase for UITitles {
    fn update(&mut self, app_data: &mut AppData, buttons: u32) {
        // update icons texture
        UITitles::update_icons(self, app_data);
        if self.save_menu.is_forces() {
            self.save_menu.update(buttons);
        } else if self.game_menu.is_forces() {
            self.game_menu.update(
                buttons,
                app_data
                    .titles
                    .get_title_by_idx(self.selected_idx)
                    .expect("selected title"),
                &app_data.titles,
            );
        } else {
            if app_data.titles.size() > 0 {
                // open save menu
                if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
                    self.save_menu.open(
                        app_data
                            .titles
                            .get_title_by_idx(self.selected_idx)
                            .expect("selected title"),
                    );
                } else if is_button(buttons, SceCtrlButtons::SceCtrlTriangle) {
                    self.game_menu.open();
                }
            }
            if is_button(buttons, SceCtrlButtons::SceCtrlSquare) {
                UIDialog::present_about(ABOUT_TEXT);
            }
            // update selected title icon
            UITitles::update_selected(self, app_data, buttons);
        }

        // free save menu
        if !self.save_menu.is_active() {
            self.save_menu.free_list();
        }
        if !self.game_menu.is_active() {
            self.game_menu.free();
        }
    }

    fn draw(&self, app_data: &AppData) {
        // select game info
        self.draw_selected_game_info(app_data);
        // game icon list
        self.draw_game_list(app_data);
        // menu
        self.draw_menu();
    }

    fn is_forces(&self) -> bool {
        self.save_menu.is_forces() || self.game_menu.is_forces()
    }
}
