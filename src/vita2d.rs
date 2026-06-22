use std::ffi::{c_char, c_float, c_int, c_uint, c_ulong, c_void};

use crate::utils::str_to_c_str;

pub enum SceCtrlButtons {
    SceCtrlSelect = 0x00000001,      //< Select button.
    SceCtrlL3 = 0x00000002,          //< L3 button.
    SceCtrlR3 = 0x00000004,          //< R3 button.
    SceCtrlStart = 0x00000008,       //< Start button.
    SceCtrlUp = 0x00000010,          //< Up D-Pad button.
    SceCtrlRight = 0x00000020,       //< Right D-Pad button.
    SceCtrlDown = 0x00000040,        //< Down D-Pad button.
    SceCtrlLeft = 0x00000080,        //< Left D-Pad button.
    SceCtrlLtrigger = 0x00000100,    //< Left trigger.
    SceCtrlRtrigger = 0x00000200,    //< Right trigger.
    SceCtrlL1 = 0x00000400,          //< L1 button.
    SceCtrlR1 = 0x00000800,          //< R1 button.
    SceCtrlTriangle = 0x00001000,    //< Triangle button.
    SceCtrlCircle = 0x00002000,      //< Circle button.
    SceCtrlCross = 0x00004000,       //< Cross button.
    SceCtrlSquare = 0x00008000,      //< Square button.
    SceCtrlIntercepted = 0x00010000, //< Input not available because intercepted by another application
    SceCtrlHeadphone = 0x00080000,   //< Headphone plugged in.
    SceCtrlVolup = 0x00100000,       //< Volume up button.
    SceCtrlVoldown = 0x00200000,     //< Volume down button.
    SceCtrlPower = 0x40000000,       //< Power button.
}

extern "C" {
    fn v2d_init();
    fn v2d_exit();
    fn v2d_free_texture(data: *mut c_void);
    fn v2d_load_png(path: *const c_char) -> *mut c_void;
    fn v2d_load_png_buf(buf: *const c_void) -> *mut c_void;
    fn v2d_load_jpg(path: *const c_char) -> *mut c_void;
    fn v2d_load_jpg_buf(buf: *const c_void, size: c_ulong) -> *mut c_void;
    fn v2d_color(r: c_int, g: c_int, b: c_int, a: c_int) -> c_uint;
    fn v2d_draw_texture(texture: *const c_void, x: c_float, y: c_float);
    fn v2d_draw_texture_scale(
        texture: *const c_void,
        x: c_float,
        y: c_float,
        sx: c_float,
        sy: c_float,
    );
    fn v2d_draw_text(x: c_int, y: c_int, color: c_uint, scale: c_float, text: *const c_char);
    fn v2d_text_width(scale: c_float, text: *const c_char) -> c_int;
    fn v2d_text_height(scale: c_float, text: *const c_char) -> c_int;
    fn v2d_ctrl_peek_positive() -> c_uint;
    fn v2d_get_screenshot() -> *mut c_void;

    // vita2d
    fn vita2d_start_drawing();
    fn vita2d_clear_screen();
    fn vita2d_end_drawing();
    fn vita2d_swap_buffers();
    fn vita2d_wait_rendering_done();
    fn vita2d_draw_rectangle(x: c_float, y: c_float, w: c_float, h: c_float, color: c_uint);
    fn vita2d_draw_line(x0: c_float, y0: c_float, x1: c_float, y1: c_float, color: c_uint);
    fn vita2d_set_blend_mode_add(enable: c_int);
    fn vita2d_enable_clipping();
    fn vita2d_disable_clipping();
    fn vita2d_set_clip_rectangle(x_min: c_int, y_min: c_int, x_max: c_int, y_max: c_int);
}

pub struct Vita2dTexture {
    tex: *mut c_void,
}

impl Vita2dTexture {
    pub fn new(tex: *mut c_void) -> Vita2dTexture {
        Vita2dTexture { tex }
    }
}

impl Drop for Vita2dTexture {
    fn drop(&mut self) {
        if !self.tex.is_null() {
            unsafe {
                v2d_free_texture(self.tex);
            }
        }
    }
}

pub struct Vita2dContext;

impl Vita2dContext {
    pub fn new() -> Vita2dContext {
        unsafe {
            v2d_init();
        }
        Vita2dContext
    }
}

impl Drop for Vita2dContext {
    fn drop(&mut self) {
        unsafe {
            v2d_exit();
        }
    }
}

pub fn vita2d_drawing() {
    unsafe {
        vita2d_start_drawing();
        vita2d_clear_screen();
    }
}

pub fn vita2d_present() {
    unsafe {
        vita2d_end_drawing();
        vita2d_wait_rendering_done();
        vita2d_swap_buffers();
    }
}

pub fn vita2d_load_png_file(path: &str) -> Vita2dTexture {
    unsafe {
        let c_str = str_to_c_str(path);
        Vita2dTexture {
            tex: v2d_load_png(c_str.as_slice().as_ptr() as *const i8),
        }
    }
}

pub fn vita2d_load_png_buf(buf: &[u8]) -> Vita2dTexture {
    unsafe {
        Vita2dTexture {
            tex: v2d_load_png_buf(buf.as_ptr() as *const c_void),
        }
    }
}

pub fn vita2d_load_jpg_file(path: &str) -> Vita2dTexture {
    unsafe {
        let c_str = str_to_c_str(path);
        Vita2dTexture {
            tex: v2d_load_jpg(c_str.as_slice().as_ptr() as *const i8),
        }
    }
}

pub fn vita2d_load_jpg_buf(buf: &[u8]) -> Vita2dTexture {
    unsafe {
        Vita2dTexture {
            tex: v2d_load_jpg_buf(buf.as_ptr() as *const c_void, buf.len() as c_ulong),
        }
    }
}

pub fn rgba(r: i32, g: i32, b: i32, a: i32) -> u32 {
    unsafe { v2d_color(r, g, b, a) }
}

pub fn vita2d_draw_rect(x: f32, y: f32, w: f32, h: f32, color: u32) {
    unsafe {
        vita2d_draw_rectangle(x, y, w, h, color);
    }
}

pub fn vita2d_draw_texture(texture: &Vita2dTexture, x: f32, y: f32) {
    if texture.tex.is_null() {
        return;
    }
    unsafe {
        v2d_draw_texture(texture.tex, x, y);
    }
}

pub fn vita2d_draw_texture_scale(texture: &Vita2dTexture, x: f32, y: f32, sx: f32, sy: f32) {
    if texture.tex.is_null() {
        return;
    }
    unsafe {
        v2d_draw_texture_scale(texture.tex, x, y, sx, sy);
    }
}

pub fn vita2d_set_clip(x_min: i32, y_min: i32, x_max: i32, y_max: i32) {
    unsafe {
        vita2d_enable_clipping();
        vita2d_set_clip_rectangle(x_min, y_min, x_max, y_max);
    }
}

pub fn vita2d_unset_clip() {
    unsafe {
        vita2d_disable_clipping();
    }
}

pub fn vita2d_draw_text(x: i32, y: i32, color: u32, scale: f32, text: &str) {
    let c_str = str_to_c_str(text);
    unsafe {
        v2d_draw_text(x, y, color, scale, c_str.as_ptr() as *const c_char);
    }
}

pub fn vita2d_text_width(scale: f32, text: &str) -> i32 {
    let c_str = str_to_c_str(text);
    unsafe { v2d_text_width(scale, c_str.as_ptr() as *const c_char) }
}

pub fn vita2d_text_height(scale: f32, text: &str) -> i32 {
    let c_str = str_to_c_str(text);
    unsafe { v2d_text_height(scale, c_str.as_ptr() as *const c_char) }
}

pub fn vita2d_line(x0: f32, y0: f32, x1: f32, y1: f32, color: u32) {
    unsafe {
        vita2d_draw_line(x0, y0, x1, y1, color);
    }
}

pub fn vita2d_enable_blend_mode() {
    unsafe { vita2d_set_blend_mode_add(1) }
}

pub fn vita2d_disable_blend_mode() {
    unsafe { vita2d_set_blend_mode_add(0) }
}

pub fn vita2d_ctrl_peek_positive() -> u32 {
    unsafe { v2d_ctrl_peek_positive() }
}

pub fn vita2d_get_screenshot() -> Vita2dTexture {
    unsafe {
        Vita2dTexture {
            tex: v2d_get_screenshot(),
        }
    }
}

pub fn is_button(buttons: u32, button: SceCtrlButtons) -> bool {
    return buttons & button as u32 > 0;
}
