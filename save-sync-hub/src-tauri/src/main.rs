#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // WebKitGTK 2.42+ renders through DMA-BUF, which fails on several common
    // Linux driver stacks and leaves an empty white window instead of the app.
    // Opt out unless the user set the variable themselves.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    save_sync_hub_lib::run()
}
