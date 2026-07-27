use std::{env, fs, path::Path};

/// cargo-vita packages everything under assets/, so Finder's .DS_Store files
/// end up inside the VPK. They are gitignored and never reach CI, but a local
/// macOS build picks them up. Runs before packaging, so clearing them here
/// keeps them out of the bundle.
fn remove_ds_store(dir: &Path) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            remove_ds_store(&path);
        } else if path.file_name().is_some_and(|name| name == ".DS_Store") {
            let _ = fs::remove_file(&path);
        }
    }
}

fn main() {
    remove_ds_store(Path::new("assets"));

    let vitasdk = env::var("VITASDK").unwrap_or_else(|_| "/usr/local/vitasdk".to_string());
    // cc crate picks up TARGET_AR from env; set it if not already provided
    if env::var("TARGET_AR").is_err() {
        env::set_var("TARGET_AR", format!("{}/bin/arm-vita-eabi-ar", vitasdk));
    }

    println!("cargo:rustc-link-search=all=./c");
    println!(
        "cargo:rustc-link-search=all={}/arm-vita-eabi/lib",
        vitasdk
    );

    // vita2d and system stubs
    println!("cargo:rustc-link-lib=static=vita2d");
    println!("cargo:rustc-link-lib=static=SceDisplay_stub");
    println!("cargo:rustc-link-lib=static=SceGxm_stub");
    println!("cargo:rustc-link-lib=static=SceSysmodule_stub");
    println!("cargo:rustc-link-lib=static=SceCtrl_stub");
    println!("cargo:rustc-link-lib=static=ScePgf_stub");
    println!("cargo:rustc-link-lib=static=SceCommonDialog_stub");
    println!("cargo:rustc-link-lib=static=freetype");
    println!("cargo:rustc-link-lib=static=png");
    println!("cargo:rustc-link-lib=static=jpeg");
    println!("cargo:rustc-link-lib=static=z");
    println!("cargo:rustc-link-lib=static=m");
    println!("cargo:rustc-link-lib=static=c");
    println!("cargo:rustc-link-lib=static=SceAppMgr_stub");

    // tai / kernel
    println!("cargo:rustc-link-lib=static=taihen_stub");
    println!("cargo:rustc-link-lib=static=SceVshBridge_stub");
    println!("cargo:rustc-link-lib=static=SceRegistryMgr_stub");
    println!("cargo:rustc-link-lib=static=SceAppUtil_stub");

    // Use firmware SQLite (SceSqlite_stub) — same as Chinese version, gives access to ur0: VFS
    println!("cargo:rustc-link-lib=static=SceSqlite_stub");
    println!("cargo:rustc-link-lib=static=SceLibKernel_stub");
    println!("cargo:rustc-link-lib=static=VitaShellUser_stub_weak");

    // VitaShell custom SQLite VFS — registers "psp2_rw" to allow opening ur0:/shell/db/app.db
    cc::Build::new()
        .file("./c/vita_sqlite_vfs.c")
        .include("./c")
        .static_flag(true)
        .warnings(false)
        .compile("vita_sqlite_vfs");

    cc::Build::new()
        .file("./c/tai.c")
        .static_flag(true)
        .compile("tai");

    cc::Build::new()
        .file("./c/v2d.c")
        .static_flag(true)
        .compile("v2d");

    cc::Build::new()
        .file("./c/ime.c")
        .static_flag(true)
        .compile("ime");
}
