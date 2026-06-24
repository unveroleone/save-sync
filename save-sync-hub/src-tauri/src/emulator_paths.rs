pub enum Platform {
    Psp,
    RetroArch,
    Custom(String),
}

pub struct PlatformPaths {
    #[allow(dead_code)]
    pub save_dir: String,
    #[allow(dead_code)]
    pub label: String,
}

pub fn get_platform_paths(platform: &Platform) -> Option<PlatformPaths> {
    match platform {
        Platform::Psp => psp_path(),
        Platform::RetroArch => retroarch_path(),
        Platform::Custom(path) => Some(PlatformPaths {
            save_dir: path.clone(),
            label: "Custom".to_string(),
        }),
    }
}

fn psp_path() -> Option<PlatformPaths> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(PlatformPaths {
            save_dir: format!("{}/Documents/PPSSPP/PSP/SAVEDATA", home),
            label: "PSP (PPSSPP)".to_string(),
        })
    }
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var("XDG_CONFIG_HOME")
            .or_else(|_| std::env::var("HOME").map(|h| format!("{}/.config", h)))
            .ok()?;
        Some(PlatformPaths {
            save_dir: format!("{}/ppsspp/PSP/SAVEDATA", config),
            label: "PSP (PPSSPP)".to_string(),
        })
    }
    #[cfg(target_os = "windows")]
    {
        let userprofile = std::env::var("USERPROFILE").ok()?;
        Some(PlatformPaths {
            save_dir: format!("{}\\Documents\\PPSSPP\\PSP\\SAVEDATA", userprofile),
            label: "PSP (PPSSPP)".to_string(),
        })
    }
    #[cfg(target_os = "android")]
    {
        Some(PlatformPaths {
            save_dir: "/storage/emulated/0/PSP/SAVEDATA".to_string(),
            label: "PSP (PPSSPP)".to_string(),
        })
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android"
    )))]
    None
}

fn retroarch_path() -> Option<PlatformPaths> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").ok()?;
        Some(PlatformPaths {
            save_dir: format!("{}/Library/Application Support/RetroArch", home),
            label: "RetroArch".to_string(),
        })
    }
    #[cfg(target_os = "linux")]
    {
        let config = std::env::var("XDG_CONFIG_HOME")
            .or_else(|_| std::env::var("HOME").map(|h| format!("{}/.config", h)))
            .ok()?;
        Some(PlatformPaths {
            save_dir: format!("{}/retroarch", config),
            label: "RetroArch".to_string(),
        })
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PlatformPaths {
            save_dir: format!("{}\\RetroArch", appdata),
            label: "RetroArch".to_string(),
        })
    }
    #[cfg(target_os = "android")]
    {
        Some(PlatformPaths {
            save_dir: "/storage/emulated/0/RetroArch".to_string(),
            label: "RetroArch".to_string(),
        })
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "windows",
        target_os = "android"
    )))]
    None
}
