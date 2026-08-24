use std::time::Instant;

use crate::{
    constant::{BUTTON_HOLDING_DELAY, BUTTON_HOLDING_REPEAT_DELAY},
    tai::{psv_prevent_sleep, unmount_pfs, Titles},
    ui::{ui_base::UIBase, ui_desktop::UIDesktop, ui_loading::Loading, ui_titles::UITitles, ui_toast::Toast},
    utils::current_time,
    vita2d::{
        is_button, vita2d_ctrl_peek_positive, vita2d_drawing, vita2d_present, SceCtrlButtons,
    },
};

pub struct AppData {
    pub titles: Titles,
}

impl AppData {
    fn new(titles: Titles) -> AppData {
        AppData { titles }
    }
}

pub struct App {
    pub data: AppData,
    pub uis: Vec<Box<dyn UIBase>>,
}

impl App {
    pub fn new(titles: Titles) -> Self {
        App {
            data: AppData::new(titles),
            uis: vec![Box::new(UIDesktop::new(Box::new(UITitles::new())))],
        }
    }

    pub fn update(&mut self, buttons: u32) -> bool {
        match self.uis.iter_mut().find(|ui| ui.is_forces()) {
            Some(ui) => {
                ui.update(&mut self.data, buttons);
                true
            }
            None => {
                for ui in self.uis.iter_mut() {
                    ui.update(&mut self.data, buttons);
                }
                false
            }
        }
    }

    pub fn draw(&self) {
        // draw
        vita2d_drawing();

        for ui in self.uis.iter() {
            ui.draw(&self.data);
        }

        // loading
        Loading::draw();
        // toast
        Toast::draw();

        vita2d_present();
    }

    pub fn present(&mut self) {
        let mut button_first_active_at = 0;
        let mut button_active_at = 0;
        let mut buttons_pre = 0;
        let mut sleep_lock_at = Instant::now();
        'main: loop {
            // get the inputs here
            let buttons_origins = vita2d_ctrl_peek_positive();
            if buttons_pre == 0 && buttons_origins > 0 {
                button_first_active_at = current_time();
            }
            let buttons = if buttons_origins > 0
                && (buttons_pre == 0
                    || (current_time() - button_first_active_at >= BUTTON_HOLDING_DELAY
                        && current_time() - button_active_at >= BUTTON_HOLDING_REPEAT_DELAY))
            {
                button_active_at = current_time();
                buttons_origins
            } else {
                0
            };
            buttons_pre = buttons_origins;

            // if update is forces
            if self.update(buttons) {
                if sleep_lock_at.elapsed().as_secs() >= 10 {
                    psv_prevent_sleep();
                    sleep_lock_at = Instant::now();
                }
            } else if is_button(buttons, SceCtrlButtons::SceCtrlStart) {
                // exit
                break 'main;
            }
            // draw
            self.draw();
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unmount_pfs();
    }
}
