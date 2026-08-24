use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{atomic::Ordering, Arc, RwLock},
};

use log::error;

use crate::{
    app::AppData,
    config::Config,
    constant::{ABOUT_TEXT, GAME_CARD_SAVE_DIR, GAME_SAVE_DIR},
    emulator::{scan_emulator_entries, EmulatorEntry, EmulatorKind},
    sync::SyncStatus,
    sync_engine::{SyncEngine, SyncGameInfo},
    tai::Title,
    utils::get_active_color,
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_draw_texture_scale,
        vita2d_load_png_buf, vita2d_set_clip, vita2d_text_height, vita2d_text_width,
        vita2d_unset_clip, SceCtrlButtons, Vita2dTexture,
    },
};

use self::save_menu::{save_list::save_list_manage::ManageContext, SaveMenu};

use super::{ui_base::UIBase, ui_dialog::UIDialog, ui_loading::Loading, ui_toast::Toast};

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
    emulator_entries: Vec<EmulatorEntry>,
    emulators_loaded: bool,
    /// Real `app_data.titles` indices not excluded from sync; grid position
    /// is the index into this Vec, not the real Titles index.
    visible_native: Vec<i32>,
    /// Set by `invalidate()`, which has no AppData to refresh with itself.
    needs_sync_refresh: bool,
    sync_engine: SyncEngine,
}

impl UITitles {
    pub fn new() -> UITitles {
        UITitles {
            top_row: 0,
            selected_idx: 0,
            icons: HashMap::new(),
            icon_bufs: Arc::new(RwLock::new(HashMap::new())),
            save_menu: SaveMenu::new(),
            emulator_entries: Vec::new(),
            emulators_loaded: false,
            visible_native: Vec::new(),
            needs_sync_refresh: false,
            sync_engine: SyncEngine::new(),
        }
    }

    /// SyncGameInfo for the currently selected grid cell, if the background
    /// fetch has reached it yet.
    fn current_sync_game(&self, app_data: &AppData) -> Option<SyncGameInfo> {
        let id = if self.selected_idx < self.native_count() {
            self.native_title(app_data, self.selected_idx)?
                .title_id()
                .to_string()
        } else {
            let emu_idx = (self.selected_idx - self.native_count()) as usize;
            self.emulator_entries.get(emu_idx)?.id.clone()
        };
        self.sync_engine
            .games
            .read()
            .unwrap()
            .iter()
            .find(|g| g.title_id == id)
            .cloned()
    }

    fn filter_excluded_emulators(entries: Vec<EmulatorEntry>, config: &Config) -> Vec<EmulatorEntry> {
        entries
            .into_iter()
            .filter(|e| !config.is_effectively_excluded(&e.id, Some(e.kind)))
            .collect()
    }

    /// Rebuilds `emulator_entries` and `visible_native` against the current
    /// exclusion config.
    fn refresh_sync_filters(&mut self, app_data: &AppData) {
        let config = Config::global();
        self.emulator_entries = Self::filter_excluded_emulators(scan_emulator_entries(), &config);
        self.visible_native = (0..app_data.titles.size() as i32)
            .filter(|&i| {
                app_data
                    .titles
                    .get_title_by_idx(i)
                    .map(|t| !config.is_effectively_excluded(t.title_id(), None))
                    .unwrap_or(false)
            })
            .collect();
        self.icons.clear();
        self.icon_bufs.write().unwrap().clear();
        self.sync_engine.games.write().unwrap().clear();
        let total = self.total_size(app_data);
        self.selected_idx = self.selected_idx.min((total - 1).max(0));
    }

    fn native_count(&self) -> i32 {
        self.visible_native.len() as i32
    }

    /// Native title at grid position `visible_idx` (not a real Titles index).
    fn native_title<'a>(&self, app_data: &'a AppData, visible_idx: i32) -> Option<&'a Title> {
        let real_idx = *self.visible_native.get(visible_idx as usize)?;
        app_data.titles.get_title_by_idx(real_idx)
    }

    fn total_size(&self, _app_data: &AppData) -> i32 {
        self.native_count() + self.emulator_entries.len() as i32
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
        let native_count = self.native_count();
        let total = self.total_size(app_data);
        let start_idx = (self.top_row - 1) * ICON_COL;
        let start_idx = if start_idx < 0 { 0 } else { start_idx };
        let end_idx = start_idx + ICON_COL * (ICON_ROW + 2);
        let end_idx = if end_idx < total { end_idx } else { total };

        // load native title icons (grid position, not the real Titles index)
        for idx in 0..self.visible_native.len() {
            let Some(title) = self.native_title(app_data, idx as i32) else {
                continue;
            };
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
                    // spawn_blocking: a plain fs::read would otherwise tie up
                    // one of the few async worker threads for its duration.
                    tokio::task::spawn_blocking(move || {
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
                        tokio::task::spawn_blocking(move || {
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
        let native_count = self.native_count();
        let total = self.total_size(app_data);
        if total == 0 {
            return;
        }

        let left = 330;
        let num = format!("→ {}/{}", self.selected_idx + 1, total);

        if self.selected_idx < native_count {
            let Some(title) = self.native_title(app_data, self.selected_idx) else {
                return;
            };
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

    /// Colored badge in a cell's corner, word-wrapped onto up to 2 short
    /// lines so it reads on sight without a legend, even in an 86px cell.
    fn draw_sync_badge(x: i32, y: i32, cell_size: i32, status: &SyncStatus, checking: bool) {
        let lines: &[&str] = if checking {
            &["Checking"]
        } else {
            match status {
                SyncStatus::InSync => &["Synced"],
                SyncStatus::UploadNeeded | SyncStatus::LocalOnly => &["Upload", "Needed"],
                SyncStatus::DownloadAvailable | SyncStatus::CloudOnly => &["Download", "Needed"],
                SyncStatus::Conflict => &["Conflict"],
            }
        };
        let scale = 0.9;
        let metrics: Vec<(i32, i32)> = lines
            .iter()
            .map(|l| (vita2d_text_width(scale, l), vita2d_text_height(scale, l)))
            .collect();
        let bw = metrics.iter().map(|(w, _)| *w).max().unwrap_or(0) + 6;
        let bh: i32 = metrics.iter().map(|(_, h)| h + 2).sum::<i32>() + 2;
        let bx = x + cell_size - bw;
        let by = y;
        // Translucent so the icon underneath still shows through. Neutral
        // gray while checking so it never flashes a status color that's
        // about to be overwritten.
        let badge_color = if checking {
            rgba(0x77, 0x77, 0x77, 0xd0)
        } else {
            SyncEngine::status_color_alpha(status, 0xd0)
        };
        vita2d_draw_rect(bx as f32, by as f32, bw as f32, bh as f32, badge_color);
        let mut cursor_y = by + 1;
        for (line, (w, h)) in lines.iter().zip(metrics.iter()) {
            cursor_y += h;
            vita2d_draw_text(bx + (bw - w) / 2, cursor_y, rgba(0xff, 0xff, 0xff, 0xff), scale, line);
            cursor_y += 2;
        }
    }

    pub fn draw_game_list(&self, app_data: &AppData) {
        let icon_bg = rgba(0x44, 0x44, 0x44, 0xff);
        let native_count = self.native_count();
        let total = self.total_size(app_data);
        let start_idx = self.top_row * ICON_COL;
        let end_idx = (start_idx + ICON_COL * ICON_ROW).min(total);
        let sync_games = self.sync_engine.games.read().unwrap();
        let status_by_id: HashMap<&str, (&SyncStatus, bool)> = sync_games
            .iter()
            .map(|g| (g.title_id.as_str(), (&g.status, g.checking)))
            .collect();

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
                if let Some(title) = self.native_title(app_data, icon_idx as i32) {
                    if let Some((status, checking)) = status_by_id.get(title.title_id()) {
                        Self::draw_sync_badge(x, y, cell_size, status, *checking);
                    }
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
                        // ICON0.PNG aspect ratio varies by platform (PSP is
                        // 144x80, PS1 is 80x80 under Adrenaline). Scale to
                        // fill the cell height using the texture's real size,
                        // then center-crop width so any ratio lands centered.
                        vita2d_draw_rect(x as f32, y as f32, cell_size as f32, cell_size as f32, icon_bg);
                        let texture = self.icons.get(&icon_idx).expect("emu icon");
                        let scale = cell_size as f32 / texture.height() as f32;
                        let draw_w = (texture.width() as f32 * scale) as i32;
                        let x_draw = x - (draw_w - cell_size) / 2;
                        vita2d_set_clip(x, y, x + cell_size, y + cell_size);
                        vita2d_draw_texture_scale(
                            texture,
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
                    if let Some((status, checking)) = status_by_id.get(entry.id.as_str()) {
                        Self::draw_sync_badge(x, y, cell_size, status, *checking);
                    }
                }
            }
        }
    }

    pub fn draw_menu(&self) {
        if self.save_menu.is_active() {
            self.save_menu.draw();
        }
    }
}

impl UIBase for UITitles {
    fn update(&mut self, app_data: &mut AppData, buttons: u32) {
        if !self.emulators_loaded {
            self.refresh_sync_filters(app_data);
            self.emulators_loaded = true;
        }

        if self.needs_sync_refresh {
            self.refresh_sync_filters(app_data);
            self.needs_sync_refresh = false;
        }

        self.sync_engine.pump();

        let native_count = self.native_count();

        // update icons texture (native titles only)
        UITitles::update_icons(self, app_data);

        if self.save_menu.is_forces() {
            self.save_menu.update(buttons);
            if self.save_menu.take_sync_exclusion_changed() {
                self.refresh_sync_filters(app_data);
            }
        } else if self.sync_engine.pending.load(Ordering::Relaxed) {
            // Sync (single or all) holds all input, so circle is free to
            // mean "stop". No confirmation dialog: it would block the main
            // loop mid-run.
            if is_button(buttons, SceCtrlButtons::SceCtrlCircle)
                && !self.sync_engine.cancel.swap(true, Ordering::Relaxed)
            {
                Toast::show("Stopping after this game...".to_string());
            }
            if !Loading::is_pending() {
                self.sync_engine.pending.store(false, Ordering::Relaxed);
            }
        } else {
            let total = self.total_size(app_data);
            if total > 0 {
                if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
                    if self.selected_idx < native_count {
                        let title = self.native_title(app_data, self.selected_idx);
                        if let Some(title) = title {
                            self.save_menu.open(title);
                        }
                    } else {
                        let emu_idx = (self.selected_idx - native_count) as usize;
                        if let Some(entry) = self.emulator_entries.get(emu_idx) {
                            let id = entry.id.clone();
                            let name = entry.name.clone();
                            let server_title = entry.server_title.clone();
                            let exclusions =
                                crate::config::Config::global().psp_exclusions_for(&entry.id);
                            self.save_menu.open_for(
                                &id,
                                &name,
                                &server_title,
                                Some(entry.save_target_excluding(&exclusions)),
                                false,
                                ManageContext::Emulator(entry.clone()),
                            );
                        }
                    }
                } else if is_button(buttons, SceCtrlButtons::SceCtrlTriangle) {
                    if let Some(game) = self.current_sync_game(app_data) {
                        self.sync_engine.per_game_action(&game);
                    } else {
                        Toast::show("Sync status not loaded yet.".to_string());
                    }
                }
            }
            if is_button(buttons, SceCtrlButtons::SceCtrlSquare) {
                UIDialog::present(ABOUT_TEXT);
            }
            if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
                self.sync_engine.sync_all();
            }
            if is_button(buttons, SceCtrlButtons::SceCtrlSelect) {
                self.refresh_sync_filters(app_data);
            }
            UITitles::update_selected(self, app_data, buttons);
        }

        // Background sync-status fetch. Non-blocking: the grid itself never
        // waits on it, badges just pop in once it lands.
        if self.sync_engine.games.read().unwrap().is_empty()
            && !self.sync_engine.pending.load(Ordering::Relaxed)
        {
            self.sync_engine.fetch(&app_data.titles);
        }

        if !self.save_menu.is_active() {
            self.save_menu.free_list();
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
        self.save_menu.is_forces() || self.sync_engine.pending.load(Ordering::Relaxed)
    }

    fn invalidate(&mut self) {
        self.needs_sync_refresh = true;
    }
}
