use crate::app::AppData;

pub trait UIBase {
    fn update(&mut self, app_data: &mut AppData, buttons: u32);
    fn draw(&self, app_data: &AppData);
    fn is_forces(&self) -> bool {
        false
    }
    fn invalidate(&mut self) {}
}
