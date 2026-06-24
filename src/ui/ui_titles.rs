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
    emulator::{scan_emulator_entries, EmulatorEntry, EmulatorKind},
    utils::get_active_color,
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_draw_texture_scale,
        vita2d_load_png_buf, vita2d_set_clip, vita2d_text_height, vita2d_text_width,
        vita2d_unset_clip, SceCtrlButtons, Vita2dTexture,
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
    emulator_entries: Vec<EmulatorEntry>,
    emulators_loaded: bool,
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
            emulator_entries: Vec::new(),
            emulators_loaded: false,
        }
    }

    fn total_size(&self, app_data: &AppData) -> i32 {
        app_data.titles.size() as i32 + self.emulator_entries.len() as i32
    }

    fn update_selected(&mut self, app_data: &mut AppData, buttons: u32) {
        let size = self.total_size(app_data);
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
        let native_count = app_data.titles.size() as i32;
        let total = self.total_size(app_data);
        let start_idx = (self.top_row - 1) * ICON_COL;
        let start_idx = if start_idx < 0 { 0 } else { start_idx };
        let end_idx = start_idx + ICON_COL * (ICON_ROW + 2);
        let end_idx = if end_idx < total { end_idx } else { total };

        // load native title icons
        for (idx, title) in app_data.titles.iter().enumerate() {
            if idx >= start_idx as usize && idx < end_idx as usize {
                let key = idx as u32;
                let has_icon = self.icons.contains_key(&key);
                if has_icon {
                    continue;
                }

                if let Ok(mut icon_bufs) = self.icon_bufs.try_write() {
                    if icon_bufs.contains_key(&key) {
                        if let Some(buf) = icon_bufs.get(&key).expect("get icon bufs") {
                            self.icons.insert(key, vita2d_load_png_buf(buf.as_slice()));
                            icon_bufs.remove(&key);
                        }
                        drop(icon_bufs);
                        continue;
                    }
                    icon_bufs.insert(key, None);
                    drop(icon_bufs);

                    let iconpath = title.iconpath().to_string();
                    let icon_bufs = Arc::clone(&self.icon_bufs);
                    tokio::spawn(async move {
                        if Path::new(&iconpath).exists() {
                            match fs::read(&iconpath) {
                                Ok(file) => {
                                    icon_bufs
                                        .write()
                                        .expect("get write lock of icon bufs in spawn")
                                        .insert(key, Some(file));
                                }
                                Err(e) => {
                                    error!("app iconpath read failed {}: {}", iconpath, e);
                                }
                            }
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

        // load emulator PSP icons
        for (emu_idx, entry) in self.emulator_entries.iter().enumerate() {
            let grid_idx = (native_count + emu_idx as i32) as u32;
            if (grid_idx as i32) >= start_idx && (grid_idx as i32) < end_idx {
                if let Some(ref icon_path) = entry.icon_path {
                    let has_icon = self.icons.contains_key(&grid_idx);
                    if has_icon {
                        continue;
                    }
                    if let Ok(mut icon_bufs) = self.icon_bufs.try_write() {
                        if icon_bufs.contains_key(&grid_idx) {
                            if let Some(buf) = icon_bufs.get(&grid_idx).expect("get icon bufs") {
                                self.icons.insert(grid_idx, vita2d_load_png_buf(buf.as_slice()));
                                icon_bufs.remove(&grid_idx);
                            }
                            drop(icon_bufs);
                            continue;
                        }
                        icon_bufs.insert(grid_idx, None);
                        drop(icon_bufs);

                        let path = icon_path.clone();
                        let icon_bufs = Arc::clone(&self.icon_bufs);
                        tokio::spawn(async move {
                            if Path::new(&path).exists() {
                                match fs::read(&path) {
                                    Ok(file) => {
                                        icon_bufs
                                            .write()
                                            .expect("get write lock of icon bufs in spawn")
                                            .insert(grid_idx, Some(file));
                                    }
                                    Err(e) => {
                                        error!("emu icon read failed {}: {}", path, e);
                                    }
                                }
                            }
                        });
                    }
                }
            }
        }
    }

    fn draw_selected_game_info(&self, app_data: &AppData) {
        let native_count = app_data.titles.size() as i32;
        let total = self.total_size(app_data);
        if total == 0 {
            return;
        }

        let left = 330;
        let num = format!("→ {}/{}", self.selected_idx + 1, total);

        if self.selected_idx < native_count {
            let titles = &app_data.titles;
            let title = titles
                .get_title_by_idx(self.selected_idx)
                .expect("get title by idx");
            let real_id = title.real_id();
            let header = format!("{}  |  {}", title.title_id(), title.name());
            let mut save_path = format!("{}/{}", GAME_CARD_SAVE_DIR, real_id);
            if !Path::new(&save_path).exists() {
                save_path = format!("{}/{}", GAME_SAVE_DIR, real_id);
            }
            vita2d_draw_text(
                left,
                10 + vita2d_text_height(1.0, &header),
                rgba(0xff, 0xff, 0xff, 0xff),
                1.0,
                &header,
            );
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
        } else {
            let emu_idx = (self.selected_idx - native_count) as usize;
            if let Some(entry) = self.emulator_entries.get(emu_idx) {
                vita2d_draw_text(
                    left,
                    10 + vita2d_text_height(1.0, &entry.name),
                    rgba(0xff, 0xff, 0xff, 0xff),
                    1.0,
                    &entry.name,
                );
                vita2d_draw_text(
                    left,
                    35 + vita2d_text_height(1.0, &entry.source_path),
                    rgba(0xaa, 0xaa, 0xaa, 0xff),
                    1.0,
                    &entry.source_path,
                );
            }
        }

        vita2d_draw_text(
            left,
            60 + vita2d_text_height(1.0, &num),
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            &num,
        );

        // selected icon bg highlight — color depends on entry type
        let highlight_color = if self.selected_idx >= native_count {
            let emu_idx = (self.selected_idx - native_count) as usize;
            match self.emulator_entries.get(emu_idx).map(|e| &e.kind) {
                Some(EmulatorKind::Psp) => rgba(0xff, 0x6b, 0x9d, 0xff),
                Some(EmulatorKind::RetroArch) => rgba(0xff, 0x77, 0x00, 0xff),
                None => get_active_color(),
            }
        } else {
            get_active_color()
        };
        vita2d_draw_rect(
            (10 + (self.selected_idx % ICON_COL) * ICON_SIZE - 3) as f32,
            100.0
                + (((self.selected_idx - self.top_row * ICON_COL) / ICON_COL) * ICON_SIZE) as f32
                - 3.0,
            100.0,
            100.0,
            highlight_color,
        );
    }

    pub fn draw_game_list(&self, app_data: &AppData) {
        let icon_bg = rgba(0x44, 0x44, 0x44, 0xff);
        let native_count = app_data.titles.size() as i32;
        let total = self.total_size(app_data);
        let start_idx = self.top_row * ICON_COL;
        let end_idx = (start_idx + ICON_COL * ICON_ROW).min(total);

        for idx in 0..(ICON_COL * ICON_ROW) as i32 {
            if start_idx + idx >= end_idx {
                continue;
            }
            let icon_idx = (start_idx + idx) as u32;
            let is_selected = icon_idx as i32 == self.selected_idx;
            let pad = if is_selected { 0 } else { 8 };
            let x = (idx % ICON_COL) * ICON_SIZE + (pad / 2) + OFFSET_LEFT;
            let y = (idx / ICON_COL) * ICON_SIZE + (pad / 2) + OFFSET_TOP;
            let cell_size = ICON_SIZE - pad;

            if (icon_idx as i32) < native_count {
                // native title cell
                vita2d_draw_rect(x as f32, y as f32, cell_size as f32, cell_size as f32, icon_bg);
                if self.icons.contains_key(&icon_idx) {
                    vita2d_draw_texture_scale(
                        self.icons.get(&icon_idx).expect("get icon texture"),
                        x as f32,
                        y as f32,
                        cell_size as f32 / 128.0,
                        cell_size as f32 / 128.0,
                    );
                }
            } else {
                // emulator cell
                let emu_idx = (icon_idx as i32 - native_count) as usize;
                if let Some(entry) = self.emulator_entries.get(emu_idx) {
                    let type_color = match entry.kind {
                        EmulatorKind::Psp => rgba(0xff, 0x6b, 0x9d, 0xff),
                        EmulatorKind::RetroArch => rgba(0xff, 0x77, 0x00, 0xff),
                    };
                    let border = 2;
                    let has_icon = self.icons.contains_key(&icon_idx);
                    if has_icon {
                        // PSP ICON0.PNG is 144x80 (16:9). Scale to fill height, center-crop width.
                        vita2d_draw_rect(x as f32, y as f32, cell_size as f32, cell_size as f32, icon_bg);
                        let scale = cell_size as f32 / 80.0;
                        let draw_w = (144.0 * scale) as i32;
                        let x_draw = x - (draw_w - cell_size) / 2;
                        vita2d_set_clip(x, y, x + cell_size, y + cell_size);
                        vita2d_draw_texture_scale(
                            self.icons.get(&icon_idx).expect("emu icon"),
                            x_draw as f32,
                            y as f32,
                            scale,
                            scale,
                        );
                        vita2d_unset_clip();
                    } else {
                        vita2d_draw_rect(x as f32, y as f32, cell_size as f32, cell_size as f32, type_color);
                        vita2d_draw_rect(
                            (x + border) as f32,
                            (y + border) as f32,
                            (cell_size - border * 2) as f32,
                            (cell_size - border * 2) as f32,
                            rgba(0x22, 0x22, 0x22, 0xff),
                        );
                        let label = match entry.kind {
                            EmulatorKind::Psp => "PSP",
                            EmulatorKind::RetroArch => "RA",
                        };
                        let lw = vita2d_text_width(1.0, label);
                        let lh = vita2d_text_height(1.0, label);
                        vita2d_draw_text(
                            x + (cell_size - lw) / 2,
                            y + (cell_size + lh) / 2,
                            rgba(0xff, 0xff, 0xff, 0xff),
                            1.0,
                            label,
                        );
                    }
                }
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
        // load emulator entries once on first update
        if !self.emulators_loaded {
            self.emulator_entries = scan_emulator_entries();
            self.emulators_loaded = true;
        }

        let native_count = app_data.titles.size() as i32;

        // update icons texture (native titles only)
        UITitles::update_icons(self, app_data);

        if self.save_menu.is_forces() {
            self.save_menu.update(buttons);
        } else if self.game_menu.is_forces() {
            // game menu is only ever opened for native titles
            if self.selected_idx < native_count {
                self.game_menu.update(
                    buttons,
                    app_data
                        .titles
                        .get_title_by_idx(self.selected_idx)
                        .expect("selected title"),
                    &app_data.titles,
                );
            }
        } else {
            let total = self.total_size(app_data);
            if total > 0 {
                if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
                    if self.selected_idx < native_count {
                        self.save_menu.open(
                            app_data
                                .titles
                                .get_title_by_idx(self.selected_idx)
                                .expect("selected title"),
                        );
                    } else {
                        let emu_idx = (self.selected_idx - native_count) as usize;
                        if let Some(entry) = self.emulator_entries.get(emu_idx) {
                            let id = entry.id.clone();
                            let name = entry.name.clone();
                            let path = entry.source_path.clone();
                            self.save_menu.open_for(&id, &name, Some(path), false);
                        }
                    }
                } else if is_button(buttons, SceCtrlButtons::SceCtrlTriangle) {
                    if self.selected_idx < native_count {
                        self.game_menu.open();
                    }
                }
            }
            if is_button(buttons, SceCtrlButtons::SceCtrlSquare) {
                UIDialog::present_about(ABOUT_TEXT);
            }
            UITitles::update_selected(self, app_data, buttons);
        }

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
