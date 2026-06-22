use crate::{
    api::Api,
    config::Config,
    constant::{SCREEN_HEIGHT, SCREEN_WIDTH},
    ime::show_keyboard,
    ui::{ui_loading::Loading, ui_toast::Toast},
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
    pub dirty: bool,
    pub should_close: bool,
}

impl UISettings {
    pub fn new(config: &Config) -> Self {
        UISettings {
            selected_idx: 0,
            config: config.clone(),
            testing: false,
            dirty: false,
            should_close: false,
        }
    }

    fn field_count(&self) -> i32 {
        5 // URL, Token, Device Name, Test Connection, Back
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
}

impl UIBase for UISettings {
    fn update(&mut self, _app_data: &mut crate::app::AppData, buttons: u32) {
        if self.testing {
            if !Loading::is_pending() {
                self.testing = false;
            }
            return;
        }

        if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
            // Close without saving
            self.should_close = true;
            self.dirty = false;
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
                        self.dirty = true;
                    }
                }
                1 => {
                    let input = show_keyboard(&self.config.api_token);
                    if !input.is_empty() {
                        self.config.api_token = input.to_string();
                        self.dirty = true;
                    }
                }
                2 => {
                    let input = show_keyboard(&self.config.device_name);
                    if !input.is_empty() {
                        self.config.device_name = input.to_string();
                        self.dirty = true;
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
                    self.dirty = true;
                    self.should_close = true;
                }
                _ => {}
            }
        }
    }

    fn draw(&self, _app_data: &crate::app::AppData) {
        // cover the tab bar and background drawn by UIDesktop
        vita2d_draw_rect(0.0, 0.0, SCREEN_WIDTH as f32, SCREEN_HEIGHT as f32, rgba(0x10, 0x10, 0x10, 0xff));

        // header
        let title = "Settings";
        vita2d_draw_text(
            (SCREEN_WIDTH - vita2d_text_width(1.0, title)) / 2,
            40,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            title,
        );
        vita2d_line(0.0, 60.0, SCREEN_WIDTH as f32, 60.0, rgba(0x66, 0x66, 0x66, 0xff));

        self.draw_field(0, "Server URL", &self.config.server_url, true);
        self.draw_field(1, "API Token", &self.mask_token(), true);
        self.draw_field(2, "Device Name", &self.config.device_name, true);

        // Test connection button
        let x = 12;
        let y = 100 + 44 * 3;
        if self.selected_idx == 3 {
            vita2d_draw_rect(
                x as f32,
                y as f32,
                (SCREEN_WIDTH - 24) as f32,
                42.0,
                rgba(0x44, 0x44, 0x44, 0xff),
            );
        }
        vita2d_draw_text(x + 8, y + 22, rgba(0x00, 0xb4, 0xd8, 0xff), 1.0, "Test Connection");

        // Back
        let y = 100 + 44 * 4;
        if self.selected_idx == 4 {
            vita2d_draw_rect(
                x as f32,
                y as f32,
                (SCREEN_WIDTH - 24) as f32,
                42.0,
                rgba(0x44, 0x44, 0x44, 0xff),
            );
        }
        vita2d_draw_text(x + 8, y + 22, rgba(0xff, 0x88, 0x88, 0xff), 1.0, "Save && Back");

        // Bottom bar
        let bar = "(X) Edit    (O) Back";
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

        self.draw_testing_overlay();
    }

    fn is_forces(&self) -> bool {
        true
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
