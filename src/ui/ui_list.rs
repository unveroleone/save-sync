use crate::utils::SaveTarget;

pub trait UIList {
    fn init(&mut self);

    fn is_pending(&self) -> bool;

    fn do_restore_game_save(&self, _save_target: &Option<SaveTarget>, _backup_name: &str) {}

    fn do_backup_game_save(&self, save_target: &Option<SaveTarget>, input: Option<String>);

    fn do_delete_game_save(&self, backup_name: &str);

    fn update(&mut self, save_target: &Option<SaveTarget>, buttons: u32);

    fn draw(&self, left: i32, top: i32);
}
