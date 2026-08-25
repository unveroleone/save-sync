use std::{
    path::Path,
    sync::{Arc, RwLock},
};

use crate::{
    config::Config,
    constant::{
        FOLDER_PICKER_BOTTOM_BAR_TEXT, GAME_CARD_SAVE_DIR, GAME_SAVE_DIR, NEW_BACKUP,
        SAVE_DRAWER_BOTTOM_BAR_TEXT, SAVE_DRAWER_MANAGE_BOTTOM_BAR_TEXT, SCREEN_WIDTH, TAB_LOCAL,
        TAB_MANAGE, TEXT_L, TEXT_R,
    },
    tai::Title,
    ui::{ui_drawer::UIDrawer, ui_list::UIList},
    utils::SaveTarget,
    vita2d::{
        is_button, rgba, vita2d_draw_rect, vita2d_draw_text, vita2d_line, vita2d_text_height,
        vita2d_text_width, SceCtrlButtons,
    },
};

use self::save_list::{
    save_list_local::SaveListLocal,
    save_list_manage::{ManageContext, SaveListManage},
};

pub mod save_list;

pub struct SaveMenu {
    is_list_local: bool,
    local: Option<Box<dyn UIList>>,
    manage: Option<Box<dyn UIList>>,
    drawer: Option<UIDrawer>,
    save_target: Option<SaveTarget>,
}

impl SaveMenu {
    pub fn new() -> SaveMenu {
        SaveMenu {
            is_list_local: true,
            local: None,
            manage: None,
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
        manage_context: ManageContext,
    ) {
        self.save_target = save_target;
        let config = Arc::new(RwLock::new(Config::global()));
        self.local = Some(Box::new(SaveListLocal::new(
            NEW_BACKUP,
            title_id,
            name,
            server_title,
            needs_pfs,
            config,
        )));
        self.manage = Some(Box::new(SaveListManage::new(
            title_id,
            name,
            server_title,
            needs_pfs,
            manage_context,
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
        if !self.manage.is_none() {
            self.manage = None;
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
        self.open_for(
            title.title_id(),
            title.name(),
            title.name(),
            save_target,
            true,
            ManageContext::Native {
                real_id: title.real_id().to_string(),
            },
        );
    }

    pub fn close(&mut self) {
        if let Some(drawer) = &mut self.drawer {
            drawer.close();
        }
    }

    pub fn is_pending(&self) -> bool {
        // check SaveList is pending
        [&self.local, &self.manage]
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
            &mut self.manage
        }
    }

    pub fn init_save_list(&mut self) {
        if let Some(save_list) = &mut self.get_save_list() {
            save_list.init();
        }
    }

    /// True (once) if an exclusion changed since the last check.
    pub fn take_sync_exclusion_changed(&mut self) -> bool {
        self.manage
            .as_mut()
            .map(|list| list.take_sync_exclusion_changed())
            .unwrap_or(false)
    }

    /// The title_id (once) of an entry whose server-side state changed via
    /// a Manage-tab action, if any.
    pub fn take_needs_single_refresh(&mut self) -> Option<String> {
        self.manage.as_mut().and_then(|list| list.take_needs_single_refresh())
    }

    pub fn update(&mut self, buttons: u32) {
        if self.is_pending() {
            return;
        }

        // While a sub-view (e.g. the PSP folder picker) owns input, circle
        // means "save it", not "close the whole drawer".
        let picker_active = self
            .get_save_list()
            .as_ref()
            .map(|list| list.picker_active())
            .unwrap_or(false);
        if picker_active {
            let save_target = self.save_target.clone();
            if let Some(save_list) = self.get_save_list() {
                save_list.update(&save_target, buttons);
            }
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
        } else {
            let save_target = self.save_target.clone();
            if let Some(save_list) = self.get_save_list() {
                save_list.update(&save_target, buttons);
            }
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
        // manage
        vita2d_draw_text(
            left + (SCREEN_WIDTH / 4)
                + ((SCREEN_WIDTH / 4 - 12) - vita2d_text_width(1.0, TAB_MANAGE)) / 2,
            5 + 22,
            rgba(0xff, 0xff, 0xff, 0xff),
            1.0,
            TAB_MANAGE,
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
                drawer.draw(SAVE_DRAWER_BOTTOM_BAR_TEXT);
                self.draw_tabs(left);
                if let Some(local) = &self.local {
                    local.draw(left, 10);
                }
            } else {
                let picker_active = self
                    .manage
                    .as_ref()
                    .map(|list| list.picker_active())
                    .unwrap_or(false);
                let bar_text = if picker_active {
                    FOLDER_PICKER_BOTTOM_BAR_TEXT
                } else {
                    SAVE_DRAWER_MANAGE_BOTTOM_BAR_TEXT
                };
                drawer.draw(bar_text);
                if !picker_active {
                    self.draw_tabs(left);
                }
                if let Some(manage) = &self.manage {
                    manage.draw(left, 10);
                }
            }
        }
    }
}
