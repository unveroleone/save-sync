use crate::vita2d::{is_button, SceCtrlButtons};

pub struct ListState {
    pub top_row: i32,
    pub selected_idx: i32,
    pub display_row: i32,
}

impl ListState {
    pub fn new(display_row: i32) -> ListState {
        ListState {
            top_row: 0,
            selected_idx: 0,
            display_row,
        }
    }

    pub fn do_scroll(&mut self, size: i32, buttons: u32) {
        // selected_idx
        if is_button(buttons, SceCtrlButtons::SceCtrlUp) {
            self.selected_idx -= 1;
        } else if is_button(buttons, SceCtrlButtons::SceCtrlDown) {
            self.selected_idx += 1;
        }

        // selected_idx scope check
        if self.selected_idx < 0 {
            self.selected_idx = if size > 0 { size - 1 } else { 0 };
        } else if self.selected_idx >= size {
            self.selected_idx = 0;
        }
        // top_row scope check
        if self.selected_idx < self.top_row {
            self.top_row = self.selected_idx;
        } else if self.selected_idx - self.top_row >= self.display_row {
            self.top_row = self.selected_idx - self.display_row + 1;
        }
    }

    pub fn update(&mut self, size: i32, buttons: u32) {
        // update scroll
        let idx = self.selected_idx;
        if is_button(buttons, SceCtrlButtons::SceCtrlUp)
            || is_button(buttons, SceCtrlButtons::SceCtrlDown)
            || idx >= size
        {
            if idx >= size {
                self.selected_idx = size - 1;
            }
            self.do_scroll(size, buttons);
        }
    }

    /// Back to the first row. Call after swapping the backing list, or an
    /// index from the longer list can outlive it and index out of bounds.
    pub fn reset(&mut self) {
        self.top_row = 0;
        self.selected_idx = 0;
    }
}
