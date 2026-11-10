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

    /// True (once) if an exclusion changed since the last check — the
    /// grid's membership itself may have changed, so this calls for a full
    /// re-scan, not just one entry's status.
    fn take_sync_exclusion_changed(&mut self) -> bool {
        false
    }

    /// The title_id (once) of an entry whose server-side state changed via
    /// a Manage-tab action (upload/restore/delete-from-server, or a local
    /// delete) — membership doesn't change, so only that one entry's
    /// status needs recomputing, not every entry's.
    fn take_needs_single_refresh(&mut self) -> Option<String> {
        None
    }
}
