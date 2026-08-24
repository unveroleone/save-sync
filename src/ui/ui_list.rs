use crate::utils::SaveTarget;

pub trait UIList {
    fn init(&mut self);

    fn is_pending(&self) -> bool;

    fn do_restore_game_save(&self, _save_target: &Option<SaveTarget>, _backup_name: &str) {}

    fn do_backup_game_save(&self, save_target: &Option<SaveTarget>, input: Option<String>);

    fn do_delete_game_save(&self, backup_name: &str);

    fn update(&mut self, save_target: &Option<SaveTarget>, buttons: u32);

    fn draw(&self, left: i32, top: i32);

    /// True while a nested sub-view (e.g. a folder picker) owns input, so the
    /// drawer's own circle-to-close must be deferred to it.
    fn picker_active(&self) -> bool {
        false
    }

    fn sync_exclusion_changed(&self) -> bool {
        false
    }

    fn take_sync_exclusion_changed(&mut self) -> bool {
        false
    }
}
