use crate::{
    api::Api,
    config::Config,
    constant::{SCREEN_HEIGHT, SCREEN_WIDTH},
    ime::show_keyboard,
    ui::{list_state::ListState, ui_loading::Loading, ui_toast::Toast},
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_line, vita2d_text_height,
        vita2d_text_width, SceCtrlButtons,
    },
};

use super::ui_base::UIBase;

pub struct UISettings {
    selected_idx: i32,
    config: Config,
    testing: bool,
    loading_devices: bool,
    pub should_close: bool,
    /// Nested sub-screen: every known game, grouped under its platform's
    /// category toggle.
    exclusions_view: bool,
    exclusions_list: ListState,
    native_games: Vec<(String, String)>,
    psp_games: Vec<(String, String)>,
    retroarch_games: Vec<(String, String)>,
    games_loaded: bool,
    collapsed_native: bool,
    collapsed_psp: bool,
    collapsed_retroarch: bool,
    /// Cached; rebuilt on load or fold toggle, not every frame.
    exclusion_rows: Vec<ExclusionRow>,
    total_excluded_count: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum SettingsCategory {
    Native,
    Psp,
    RetroArch,
}

enum ExclusionRow {
    Category(SettingsCategory),
    Game(SettingsCategory, String, String), // (category, entry id, name)
}

impl UISettings {
    fn commit(&self) {
        Config::commit(self.config.clone());
    }

    pub fn new(config: &Config) -> Self {
        UISettings {
            selected_idx: 0,
            config: config.clone(),
            testing: false,
            loading_devices: false,
            should_close: false,
            exclusions_view: false,
            exclusions_list: ListState::new(10),
            native_games: Vec::new(),
            psp_games: Vec::new(),
            retroarch_games: Vec::new(),
            games_loaded: false,
            collapsed_native: false,
            collapsed_psp: false,
            collapsed_retroarch: false,
            exclusion_rows: Vec::new(),
            total_excluded_count: 0,
        }
    }

    fn is_collapsed(&self, category: SettingsCategory) -> bool {
        match category {
            SettingsCategory::Native => self.collapsed_native,
            SettingsCategory::Psp => self.collapsed_psp,
            SettingsCategory::RetroArch => self.collapsed_retroarch,
        }
    }

    fn set_collapsed(&mut self, category: SettingsCategory, collapsed: bool) {
        match category {
            SettingsCategory::Native => self.collapsed_native = collapsed,
            SettingsCategory::Psp => self.collapsed_psp = collapsed,
            SettingsCategory::RetroArch => self.collapsed_retroarch = collapsed,
        }
        self.rebuild_exclusion_rows();
    }

    fn toggle_collapsed(&mut self, category: SettingsCategory) {
        self.set_collapsed(category, !self.is_collapsed(category));
    }

    fn field_count(&self) -> i32 {
        6 // URL, Token, Device Name, Test Connection, Connected Devices, Sync Exclusions
    }

    fn load_all_games(&mut self, app_data: &crate::app::AppData) {
        self.native_games = app_data
            .titles
            .iter()
            .map(|t| (t.title_id().to_string(), t.name().to_string()))
            .collect();
        self.psp_games.clear();
        self.retroarch_games.clear();
        for e in crate::emulator::scan_emulator_entries() {
            match e.kind {
                crate::emulator::EmulatorKind::Psp => self.psp_games.push((e.id, e.name)),
                crate::emulator::EmulatorKind::RetroArch => self.retroarch_games.push((e.id, e.name)),
            }
        }
        // Already-excluded categories start folded.
        self.collapsed_native = self.config.sync_exclude_all_native;
        self.collapsed_psp = self.config.sync_exclude_all_psp;
        self.collapsed_retroarch = self.config.sync_exclude_all_retroarch;
        self.rebuild_exclusion_rows();
        self.recompute_total_excluded();
    }

    fn rebuild_exclusion_rows(&mut self) {
        let mut rows = Vec::new();
        for (category, games) in [
            (SettingsCategory::Native, &self.native_games),
            (SettingsCategory::Psp, &self.psp_games),
            (SettingsCategory::RetroArch, &self.retroarch_games),
        ] {
            rows.push(ExclusionRow::Category(category));
            if !self.is_collapsed(category) {
                rows.extend(
                    games
                        .iter()
                        .map(|(id, name)| ExclusionRow::Game(category, id.clone(), name.clone())),
                );
            }
        }
        self.exclusion_rows = rows;
    }

    fn recompute_total_excluded(&mut self) {
        let count = |games: &[(String, String)], category_excluded: bool| {
            if category_excluded {
                games.len()
            } else {
                games
                    .iter()
                    .filter(|(id, _)| self.config.is_sync_excluded(id))
                    .count()
            }
        };
        self.total_excluded_count = count(&self.native_games, self.config.sync_exclude_all_native)
            + count(&self.psp_games, self.config.sync_exclude_all_psp)
            + count(&self.retroarch_games, self.config.sync_exclude_all_retroarch);
    }

    fn category_flag(&self, category: SettingsCategory) -> bool {
        match category {
            SettingsCategory::Native => self.config.sync_exclude_all_native,
            SettingsCategory::Psp => self.config.sync_exclude_all_psp,
            SettingsCategory::RetroArch => self.config.sync_exclude_all_retroarch,
        }
    }

    fn set_category_flag(&mut self, category: SettingsCategory, excluded: bool) {
        match category {
            SettingsCategory::Native => self.config.sync_exclude_all_native = excluded,
            SettingsCategory::Psp => self.config.sync_exclude_all_psp = excluded,
            SettingsCategory::RetroArch => self.config.sync_exclude_all_retroarch = excluded,
        }
        self.set_collapsed(category, excluded);
        if !excluded {
            let games = match category {
                SettingsCategory::Native => &self.native_games,
                SettingsCategory::Psp => &self.psp_games,
                SettingsCategory::RetroArch => &self.retroarch_games,
            };
            for (id, _) in games {
                self.config.sync_excluded_entries.remove(id);
            }
        }
        self.recompute_total_excluded();
    }

    fn draw_field(&self, idx: i32, label: &str, value: &str, _is_editable: bool) {
        let x = 12;
        let y = 100 + 44 * idx;
        let w = SCREEN_WIDTH - 24;

        // selection highlight
        if idx == self.selected_idx {
            vita2d_draw_rect(x as f32, y as f32, w as f32, 42.0, rgba(0x44, 0x44, 0x44, 0xff));
        }

        // label
        vita2d_draw_text(x + 8, y + 22, rgba(0xaa, 0xaa, 0xaa, 0xff), 1.0, label);

        // value
        let display_val = if value.is_empty() { "(not set)" } else { value };
        let val_color = if value.is_empty() {
            rgba(0x88, 0x44, 0x44, 0xff)
        } else {
            rgba(0xff, 0xff, 0xff, 0xff)
        };
        let val_x = x + w - 16 - vita2d_text_width(1.0, display_val);
        vita2d_draw_text(val_x, y + 22, val_color, 1.0, display_val);
    }

    fn update_exclusions_view(&mut self, buttons: u32) {
        if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
            self.exclusions_view = false;
            return;
        }
        let size = self.exclusion_rows.len() as i32;

        // Left/Right: page through a large category faster than Up/Down.
        if is_button(buttons, SceCtrlButtons::SceCtrlLeft) {
            let target = self.exclusions_list.selected_idx - self.exclusions_list.display_row;
            self.exclusions_list.jump_to(target, size);
        } else if is_button(buttons, SceCtrlButtons::SceCtrlRight) {
            let target = self.exclusions_list.selected_idx + self.exclusions_list.display_row;
            self.exclusions_list.jump_to(target, size);
        } else {
            self.exclusions_list.update(size, buttons);
        }

        // Fold works from any row inside the category, not just its header.
        if is_button(buttons, SceCtrlButtons::SceCtrlTriangle) {
            let category = match self.exclusion_rows.get(self.exclusions_list.selected_idx as usize) {
                Some(ExclusionRow::Category(c)) => Some(*c),
                Some(ExclusionRow::Game(c, _, _)) => Some(*c),
                None => None,
            };
            if let Some(category) = category {
                self.toggle_collapsed(category);
                if let Some(idx) = self
                    .exclusion_rows
                    .iter()
                    .position(|r| matches!(r, ExclusionRow::Category(c) if *c == category))
                {
                    self.exclusions_list
                        .jump_to(idx as i32, self.exclusion_rows.len() as i32);
                }
            }
        }

        if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
            match self.exclusion_rows.get(self.exclusions_list.selected_idx as usize) {
                Some(ExclusionRow::Category(category)) => {
                    let category = *category;
                    let excluded = !self.category_flag(category);
                    self.set_category_flag(category, excluded);
                    self.commit();
                }
                Some(ExclusionRow::Game(category, id, _)) => {
                    // Locked while the category covers it.
                    if !self.category_flag(*category) {
                        let id = id.clone();
                        let excluded = !self.config.is_sync_excluded(&id);
                        self.config.set_sync_excluded(&id, excluded);
                        self.recompute_total_excluded();
                        self.commit();
                    }
                }
                None => {}
            }
        }
    }

    fn draw_exclusions_view(&self) {
        vita2d_draw_rect(0.0, 0.0, SCREEN_WIDTH as f32, SCREEN_HEIGHT as f32, rgba(0x10, 0x10, 0x10, 0xff));

        let title = "Sync Exclusions";
        vita2d_draw_text(
            (SCREEN_WIDTH - vita2d_text_width(1.0, title)) / 2,
            40,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            title,
        );
        vita2d_line(0.0, 60.0, SCREEN_WIDTH as f32, 60.0, rgba(0x66, 0x66, 0x66, 0xff));

        let rows = &self.exclusion_rows;
        let x = 12;
        let w = SCREEN_WIDTH - 24;
        let row_h = 34;
        let ListState {
            top_row,
            selected_idx,
            display_row,
        } = self.exclusions_list;
        for row in 0..display_row {
            let i = top_row + row;
            if i as usize >= rows.len() {
                break;
            }
            let y = 100 + row_h * row;
            if i == selected_idx {
                vita2d_draw_rect(x as f32, y as f32, w as f32, (row_h - 2) as f32, rgba(0x44, 0x44, 0x44, 0xff));
            }
            let (indent, fold, label, checked) = match &rows[i as usize] {
                ExclusionRow::Category(category) => {
                    let count = match category {
                        SettingsCategory::Native => self.native_games.len(),
                        SettingsCategory::Psp => self.psp_games.len(),
                        SettingsCategory::RetroArch => self.retroarch_games.len(),
                    };
                    let name = match category {
                        SettingsCategory::Native => "All Native Vita Games",
                        SettingsCategory::Psp => "All PSP Games",
                        SettingsCategory::RetroArch => "All RetroArch Games",
                    };
                    let fold = if self.is_collapsed(*category) { "+" } else { "-" };
                    (0, fold, format!("{} ({})", name, count), self.category_flag(*category))
                }
                ExclusionRow::Game(category, id, name) => {
                    let checked = self.category_flag(*category) || self.config.is_sync_excluded(id);
                    (1, " ", name.clone(), checked)
                }
            };
            let text = format!(
                "{}{} [{}] {}",
                if indent > 0 { "   " } else { "" },
                fold,
                if checked { "x" } else { " " },
                label
            );
            let color = if checked {
                rgba(0xff, 0x88, 0x88, 0xff)
            } else {
                rgba(0xff, 0xff, 0xff, 0xff)
            };
            vita2d_draw_text(x + 8, y + 22, color, 1.0, &text);
        }

        let row_category = match rows.get(selected_idx as usize) {
            Some(ExclusionRow::Category(c)) => Some(*c),
            Some(ExclusionRow::Game(c, _, _)) => Some(*c),
            None => None,
        };
        let fold_hint = match row_category {
            Some(c) if self.is_collapsed(c) => "(△) Unfold",
            _ => "(△) Fold",
        };
        let bar = format!("{}  (X) Toggle  (Left/Right) Page  (O) Back", fold_hint);
        let bar = bar.as_str();
        vita2d_line(
            0.0,
            (SCREEN_HEIGHT - 58) as f32,
            SCREEN_WIDTH as f32,
            (SCREEN_HEIGHT - 58) as f32,
            rgba(0x99, 0x99, 0x99, 0xff),
        );
        vita2d_draw_text(
            SCREEN_WIDTH - 12 - vita2d_text_width(1.0, bar),
            SCREEN_HEIGHT - 58 / 2 + vita2d_text_height(1.0, bar) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            bar,
        );
    }

    fn draw_testing_overlay(&self) {
        if self.testing {
            let msg = "Testing connection...";
            vita2d_draw_rect(
                ((SCREEN_WIDTH - 300) / 2) as f32,
                (SCREEN_HEIGHT / 2 - 30) as f32,
                300.0,
                60.0,
                rgba(0x22, 0x22, 0x22, 0xee),
            );
            vita2d_draw_text(
                (SCREEN_WIDTH - vita2d_text_width(1.0, msg)) / 2,
                SCREEN_HEIGHT / 2 + 8,
                rgba(0xff, 0xff, 0xff, 0xff),
                1.0,
                msg,
            );
        }
        if self.loading_devices {
            let msg = "Loading devices...";
            vita2d_draw_rect(
                ((SCREEN_WIDTH - 300) / 2) as f32,
                (SCREEN_HEIGHT / 2 - 30) as f32,
                300.0,
                60.0,
                rgba(0x22, 0x22, 0x22, 0xee),
            );
            vita2d_draw_text(
                (SCREEN_WIDTH - vita2d_text_width(1.0, msg)) / 2,
                SCREEN_HEIGHT / 2 + 8,
                rgba(0xff, 0xff, 0xff, 0xff),
                1.0,
                msg,
            );
        }
    }

    fn test_connection(&mut self) {
        if self.testing {
            return;
        }
        let config = self.config.clone();
        self.testing = true;
        Loading::show();
        tokio::spawn(async move {
            let result = Api::test_connection(&config);
            Loading::hide();
            match result {
                Ok(status) => {
                    Toast::show(format!(
                        "Connected! Server v{}",
                        status.server_version
                    ));
                }
                Err(e) => {
                    Toast::show(format!("Failed: {}", e));
                }
            }
        });
    }

    fn show_devices(&mut self) {
        if self.loading_devices {
            return;
        }
        let config = self.config.clone();
        self.loading_devices = true;
        Loading::show();
        tokio::spawn(async move {
            let result = Api::get_devices(&config);
            Loading::hide();
            match result {
                Ok(devices) => {
                    if devices.is_empty() {
                        Toast::show("No devices paired yet.".to_string());
                    } else {
                        let names: Vec<String> = devices
                            .iter()
                            .map(|d| d.device_id.clone())
                            .collect();
                        Toast::show(format!("{} device(s): {}", devices.len(), names.join(", ")));
                    }
                }
                Err(e) => {
                    Toast::show(format!("Failed: {}", e));
                }
            }
        });
    }
}

impl UIBase for UISettings {
    fn update(&mut self, app_data: &mut crate::app::AppData, buttons: u32) {
        if !self.games_loaded {
            self.load_all_games(app_data);
            self.games_loaded = true;
        }

        if self.testing || self.loading_devices {
            if !Loading::is_pending() {
                self.testing = false;
                self.loading_devices = false;
            }
            return;
        }

        if self.exclusions_view {
            self.update_exclusions_view(buttons);
            return;
        }

        if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
            self.should_close = true;
            return;
        }

        if is_button(buttons, SceCtrlButtons::SceCtrlUp) {
            self.selected_idx = (self.selected_idx - 1).max(0);
        } else if is_button(buttons, SceCtrlButtons::SceCtrlDown) {
            self.selected_idx = (self.selected_idx + 1).min(self.field_count() - 1);
        } else if is_button(buttons, SceCtrlButtons::SceCtrlCross) {
            match self.selected_idx {
                0 => {
                    let input = show_keyboard(&self.config.server_url);
                    if !input.is_empty() {
                        self.config.server_url = input.to_string();
                        self.commit();
                    }
                }
                1 => {
                    let input = show_keyboard(&self.config.api_token);
                    if !input.is_empty() {
                        self.config.api_token = input.to_string();
                        self.commit();
                    }
                }
                2 => {
                    let input = show_keyboard(&self.config.device_name);
                    if !input.is_empty() {
                        self.config.device_name = input.to_string();
                        self.commit();
                    }
                }
                3 => {
                    if self.config.is_configured() {
                        self.test_connection();
                    } else {
                        Toast::show("Set server URL and token first.".to_string());
                    }
                }
                4 => {
                    if self.config.is_configured() {
                        self.show_devices();
                    } else {
                        Toast::show("Set server URL and token first.".to_string());
                    }
                }
                5 => {
                    self.exclusions_list.reset();
                    self.exclusions_view = true;
                }
                _ => {}
            }
        }
    }

    fn draw(&self, _app_data: &crate::app::AppData) {
        if self.exclusions_view {
            self.draw_exclusions_view();
            self.draw_testing_overlay();
            return;
        }

        // Renders into the same content area UIDesktop already reserves for
        // the active tab (below its top line, above its bottom bar) — no
        // full-screen cover or own header/bottom bar, those are the
        // desktop's job now that this is a tab and not a full-screen drawer.
        self.draw_field(0, "Server URL", &self.config.server_url, true);
        self.draw_field(1, "API Token", &self.mask_token(), true);
        self.draw_field(2, "Device Name", &self.config.device_name, true);

        // Test connection button
        let x = 12;
        let y_test = 100 + 44 * 3;
        if self.selected_idx == 3 {
            vita2d_draw_rect(x as f32, y_test as f32, (SCREEN_WIDTH - 24) as f32, 42.0, rgba(0x44, 0x44, 0x44, 0xff));
        }
        vita2d_draw_text(x + 8, y_test + 22, rgba(0x00, 0xb4, 0xd8, 0xff), 1.0, "Test Connection");

        // Connected Devices button
        let y_dev = 100 + 44 * 4;
        if self.selected_idx == 4 {
            vita2d_draw_rect(x as f32, y_dev as f32, (SCREEN_WIDTH - 24) as f32, 42.0, rgba(0x44, 0x44, 0x44, 0xff));
        }
        vita2d_draw_text(x + 8, y_dev + 22, rgba(0x00, 0xb4, 0xd8, 0xff), 1.0, "Connected Devices");

        // Sync Exclusions button
        let y_excl = 100 + 44 * 5;
        if self.selected_idx == 5 {
            vita2d_draw_rect(x as f32, y_excl as f32, (SCREEN_WIDTH - 24) as f32, 42.0, rgba(0x44, 0x44, 0x44, 0xff));
        }
        vita2d_draw_text(x + 8, y_excl + 22, rgba(0x00, 0xb4, 0xd8, 0xff), 1.0, "Sync Exclusions");
        if self.total_excluded_count > 0 {
            let summary = format!("{} excluded", self.total_excluded_count);
            let sw = vita2d_text_width(1.0, &summary);
            vita2d_draw_text(x + (SCREEN_WIDTH - 24) - 16 - sw, y_excl + 22, rgba(0xaa, 0xaa, 0xaa, 0xff), 1.0, &summary);
        }

        self.draw_testing_overlay();
    }

    fn is_forces(&self) -> bool {
        // At the plain field list, this behaves like a normal tab (L can
        // switch away at any time). The nested exclusions screen and the
        // async test/device calls still own input exclusively.
        self.testing || self.loading_devices || self.exclusions_view
    }
}

impl UISettings {
    fn mask_token(&self) -> String {
        if self.config.api_token.is_empty() {
            return "(not set)".to_string();
        }
        "••••••••".to_string()
    }

    pub fn get_config(&self) -> &Config {
        &self.config
    }
}
