use crate::{
    constant::{ACTION_DRAWER_BOTTOM_BAR_TEXT, FOLDER_PICKER_BOTTOM_BAR_TEXT},
    emulator::EmulatorEntry,
    tai::{Title, Titles},
    ui::ui_drawer::UIDrawer,
    vita2d::{is_button, SceCtrlButtons},
};

use self::game_list::GameList;

pub mod game_list;

pub struct GameMenu {
    list: GameList,
    drawer: Option<UIDrawer>,
}

impl GameMenu {
    pub fn new() -> GameMenu {
        GameMenu {
            list: GameList::new(),
            drawer: None,
        }
    }

    pub fn free(&mut self) {
        if !self.drawer.is_none() {
            self.drawer = None;
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

    pub fn is_pending(&self) -> bool {
        self.list.is_pending()
    }

    pub fn open(&mut self) {
        self.list.set_native();
        self.open_drawer();
    }

    pub fn open_emulator(&mut self, entry: &EmulatorEntry) {
        self.list.set_emulator(entry);
        self.open_drawer();
    }

    fn open_drawer(&mut self) {
        if self.drawer.is_none() {
            self.drawer = Some(UIDrawer::new());
        }
        if let Some(drawer) = &mut self.drawer {
            drawer.open();
        }
    }

    pub fn close(&mut self) {
        if let Some(drawer) = &mut self.drawer {
            drawer.close();
        }
    }

    pub fn update(
        &mut self,
        buttons: u32,
        title: Option<&Title>,
        titles: &Titles,
        emu: Option<&EmulatorEntry>,
    ) {
        // While the folder picker is up, circle means "save the picker", not
        // "close the menu".
        if !self.is_pending()
            && is_button(buttons, SceCtrlButtons::SceCtrlCircle)
            && !self.list.picker_active()
        {
            self.close();
        } else {
            self.list.update(buttons, title, titles, emu);
        }
    }

    pub fn draw(&self) {
        if !self.is_active() {
            return;
        }
        if let Some(drawer) = &self.drawer {
            let left = drawer.get_progress_left() as i32;
            let bar_text = if self.list.picker_active() {
                FOLDER_PICKER_BOTTOM_BAR_TEXT
            } else {
                ACTION_DRAWER_BOTTOM_BAR_TEXT
            };
            // drawer
            drawer.draw(bar_text);
            self.list.draw(left, 0);
        }
    }
}
