use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use crate::{
    config::Config,
    constant::{
        GAME_CARD_SAVE_DIR, GAME_SAVE_DIR, NEW_BACKUP, NEW_CLOUD_BACKUP,
        SAVE_DRAWER_BOTTOM_BAR_TEXT, SAVE_DRAWER_CLOUD_BOTTOM_BAR_TEXT, SCREEN_WIDTH, TAB_CLOUD,
        TAB_LOCAL, TEXT_L, TEXT_R,
    },
    tai::Title,
    ui::{ui_drawer::UIDrawer, ui_list::UIList},
    utils::SaveTarget,
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_line, vita2d_text_height,
        vita2d_text_width, SceCtrlButtons,
    },
};

use self::save_list::{save_list_cloud::SaveListCloud, save_list_local::SaveListLocal};

pub mod save_list;

pub struct SaveMenu {
    is_list_local: bool,
    local: Option<Box<dyn UIList>>,
    cloud: Option<Box<dyn UIList>>,
    drawer: Option<UIDrawer>,
    save_target: Option<SaveTarget>,
}

impl SaveMenu {
    pub fn new() -> SaveMenu {
        SaveMenu {
            is_list_local: true,
            local: None,
            cloud: None,
            drawer: None,
            save_target: None,
        }
    }

    pub fn open_for(
        &mut self,
        title_id: &str,
        name: &str,
        server_title: &str,
        save_target: Option<SaveTarget>,
        needs_pfs: bool,
    ) {
        self.save_target = save_target;
        let config = Arc::new(RwLock::new(Config::global()));
        self.local = Some(Box::new(SaveListLocal::new(
            NEW_BACKUP,
            title_id,
            name,
            server_title,
            needs_pfs,
            Arc::clone(&config),
        )));
        self.cloud = Some(Box::new(SaveListCloud::new(
            NEW_CLOUD_BACKUP,
            title_id,
            name,
            server_title,
            needs_pfs,
            config,
        )));
        self.init_save_list();
        if self.drawer.is_none() {
            self.drawer = Some(UIDrawer::new());
        }
        if let Some(drawer) = &mut self.drawer {
            drawer.open();
        }
    }

    pub fn free_list(&mut self) {
        if !self.drawer.is_none() {
            self.drawer = None;
        }
        if !self.local.is_none() {
            self.local = None;
        }
        if !self.cloud.is_none() {
            self.cloud = None;
        }
    }

    pub fn is_active(&self) -> bool {
        if let Some(drawer) = &self.drawer {
            return drawer.is_active();
        }
        false
    }

    pub fn is_forces(&self) -> bool {
        if let Some(drawer) = &self.drawer {
            return drawer.is_forces();
        }
        false
    }

    pub fn open(&mut self, title: &Title) {
        let mut save_target = None;
        for path in [
            format!("{}/{}", GAME_CARD_SAVE_DIR, title.real_id()),
            format!("{}/{}", GAME_SAVE_DIR, title.real_id()),
        ] {
            if Path::new(&path).exists() {
                save_target = Some(SaveTarget::single(&path));
                break;
            }
        }
        self.open_for(title.title_id(), title.name(), title.name(), save_target, true);
    }

    pub fn close(&mut self) {
        if let Some(drawer) = &mut self.drawer {
            drawer.close();
        }
    }

    pub fn is_pending(&self) -> bool {
        // check SaveList is pending
        [&self.local, &self.cloud]
            .iter()
            .find(|item| {
                if let Some(save_list) = item {
                    return save_list.is_pending();
                }
                false
            })
            .is_some()
    }

    pub fn get_save_list(&mut self) -> &mut Option<Box<dyn UIList>> {
        if self.is_list_local {
            &mut self.local
        } else {
            &mut self.cloud
        }
    }

    pub fn init_save_list(&mut self) {
        if let Some(save_list) = &mut self.get_save_list() {
            save_list.init();
        }
    }

    pub fn update(&mut self, buttons: u32) {
        if self.is_pending() {
            return;
        }
        if is_button(buttons, SceCtrlButtons::SceCtrlCircle) {
            self.close();
        } else if (is_button(buttons, SceCtrlButtons::SceCtrlLtrigger)
            || is_button(buttons, SceCtrlButtons::SceCtrlRtrigger))
            && !(is_button(buttons, SceCtrlButtons::SceCtrlLtrigger)
                && is_button(buttons, SceCtrlButtons::SceCtrlRtrigger))
        {
            let is_to_local = is_button(buttons, SceCtrlButtons::SceCtrlLtrigger);
            if !self.is_list_local && is_to_local {
                self.is_list_local = is_to_local;
                self.init_save_list();
            } else if self.is_list_local && !is_to_local {
                self.is_list_local = is_to_local;
                self.init_save_list();
            }
        } else if let Some(save_list) = if self.is_list_local {
            &mut self.local
        } else {
            &mut self.cloud
        } {
            save_list.update(&self.save_target, buttons);
        }
    }

    pub fn draw_tabs(&self, left: i32) {
        // active bg
        vita2d_draw_rect(
            if self.is_list_local {
                left + 12
            } else {
                left + SCREEN_WIDTH / 4
            } as f32,
            5.0,
            (SCREEN_WIDTH / 4) as f32 - 12.0,
            30.0,
            rgba(0x44, 0x44, 0x44, 0xff),
        );
        // local
        vita2d_draw_text(
            left + 12 + ((SCREEN_WIDTH / 4 - 12) - vita2d_text_width(1.0, TAB_LOCAL)) / 2,
            5 + 22,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            TAB_LOCAL,
        );
        // cloud
        vita2d_draw_text(
            left + (SCREEN_WIDTH / 4)
                + ((SCREEN_WIDTH / 4 - 12) - vita2d_text_width(1.0, TAB_CLOUD)) / 2,
            5 + 22,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            TAB_CLOUD,
        );
        // l
        vita2d_draw_text(
            left + 12,
            40 + vita2d_text_height(0.61, TEXT_L) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            0.61,
            TEXT_L,
        );
        vita2d_draw_text(
            left + SCREEN_WIDTH / 2 - 12 - vita2d_text_width(0.61, TEXT_R),
            40 + vita2d_text_height(0.61, TEXT_R) / 2,
            rgba(0xff, 0xff, 0xff, 0xff),
            0.61,
            TEXT_R,
        );
        // line
        vita2d_line(
            (left + 12) as f32,
            50.0,
            (left + SCREEN_WIDTH / 2 - 12) as f32,
            50.0,
            rgba(0x99, 0x99, 0x99, 0xff),
        );
    }

    pub fn draw(&self) {
        if !self.is_active() {
            return;
        }
        if let Some(drawer) = &self.drawer {
            let left = drawer.get_progress_left() as i32;
            if self.is_list_local {
                // drawer
                drawer.draw(SAVE_DRAWER_BOTTOM_BAR_TEXT);
                // tabs
                self.draw_tabs(left);
                // list
                if let Some(local) = &self.local {
                    local.draw(left, 10);
                }
            } else {
                // drawer
                drawer.draw(SAVE_DRAWER_CLOUD_BOTTOM_BAR_TEXT);
                // tabs
                self.draw_tabs(left);
                // list
                if let Some(cloud) = &self.cloud {
                    cloud.draw(left, 10);
                }
            }
        }
    }
}
