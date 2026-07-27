use std::{
    error::Error,
    ffi::OsStr,
    fs,
    io::{self, Read, Write},
    path::Path,
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{engine::general_purpose, Engine as _};
use log::error;

use zip::ZipWriter;

use crate::{
    constant::{BACKUP_BLACK_LIST, GAME_SAVE_LOCAL_DIR, SAVE_CLOUD_DIR},
    emulator::psp_title_prefix,
    ime::get_current_format_time,
    tai::{change_psv_account_id, get_psv_account_id},
    ui::ui_loading::Loading,
    vita2d::rgba,
};

pub fn current_time() -> u128 {
    let start = SystemTime::now();
    start
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_millis()
}

pub fn normalize_path(path: &str) -> String {
    let invalid_chars = ['\\', '/', ':', '*', '?', '"', '\'', '<', '>', '|'];
    let mut path = path.to_string();
    for c in invalid_chars.iter() {
        path = path.replace(*c, "_");
    }
    path.trim().to_string()
}

pub fn str_to_c_str(data: &str) -> Vec<u8> {
    format!("{}\0", data).into_bytes()
}

pub fn ease_out_expo(elapsed: Duration, duration: Duration, start: f32, end: f32) -> f32 {
    if elapsed >= duration {
        return end;
    }
    start
        + (end - start)
            * (1.0 - 2.0_f32.powf(-10.0 * elapsed.as_millis() as f32 / duration.as_millis() as f32))
}

pub fn get_active_color() -> u32 {
    let from = (168, 254, 255) as (i32, i32, i32);
    let to = (0, 168, 255) as (i32, i32, i32);
    let mut current = (0, 0, 0) as (i32, i32, i32);
    let p = (current_time() % 1000) as i32;

    if p < 400 {
        current.0 = from.0 + (to.0 - from.0) * p / 400;
        current.1 = from.1 + (to.1 - from.1) * p / 400;
        current.2 = from.2 + (to.2 - from.2) * p / 400;
    } else {
        current.0 = from.0 + (to.0 - from.0) * (1000 - p) / 600;
        current.1 = from.1 + (to.1 - from.1) * (1000 - p) / 600;
        current.2 = from.2 + (to.2 - from.2) * (1000 - p) / 600;
    }

    rgba(current.0, current.1, current.2, 0xff)
}

pub fn create_save_cloud_dir_if_not_exists() -> Result<(), Box<dyn Error>> {
    let path = Path::new(SAVE_CLOUD_DIR);
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    Ok(())
}

/// # get game save list of local dir
pub fn get_local_game_saves(local_dir: String, items: Arc<RwLock<Vec<String>>>) {
    let game_save_dir = Path::new(&local_dir);
    let mut list = vec![];
    for entry in game_save_dir.read_dir().expect("read game save dir") {
        if let Ok(entry) = entry {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap().to_str().unwrap();
                if name.ends_with(".zip") {
                    list.push(name.to_string());
                }
            }
        }
    }
    list.sort_by(|a, b| b.cmp(&a));
    *items.write().expect("write game saves") = list;
}

pub fn zip_dir_with(
    zip: &mut ZipWriter<fs::File>,
    input_path: &Path,
    prefix: &str,
    back_list: &[&str],
) -> Result<(), Box<dyn Error>> {
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut buffer = vec![0; 1024 * 512];
    for entry in input_path.read_dir()? {
        if let Ok(entry) = entry {
            let path = entry.path();
            let name = path.strip_prefix(Path::new(prefix)).unwrap();
            if let Some(_) = back_list.iter().find(|&&x| x == name.to_str().unwrap()) {
                continue;
            }
            Loading::notify_desc(entry.file_name().to_string_lossy().to_string());
            // Write file or directory explicitly
            // Some unzip tools unzip files with directory paths correctly, some do not!
            if path.is_file() {
                #[allow(deprecated)]
                zip.start_file_from_path(name, options)?;
                let mut input_file = fs::File::open(path)?;
                loop {
                    let size = input_file.read(&mut buffer)?;
                    if size == 0 {
                        break;
                    }
                    zip.write_all(&buffer[0..size])?;
                }
            } else if !name.as_os_str().is_empty() {
                // Only if not root! Avoids path spec / warning
                // and mapname conversion failed error on unzip
                #[allow(deprecated)]
                zip.add_directory_from_path(name, options)?;
                zip_dir_with(zip, path.as_path(), prefix, back_list)?;
            }
        }
    }

    Ok(())
}

pub fn zip_dir(from: &str, to: &str, back_list: &[&str]) -> Result<(), Box<dyn Error>> {
    let from = if from.ends_with("/") {
        from.to_string()
    } else {
        format!("{}/", from)
    };
    let output_path = Path::new(to);
    if !output_path.parent().unwrap().exists() {
        fs::create_dir_all(output_path.parent().unwrap())?;
    }
    let mut zip = zip::ZipWriter::new(fs::File::create(output_path)?);
    zip_dir_with(&mut zip, Path::new(&from), &from, back_list)?;
    zip.finish()?;
    Ok(())
}

fn zip_dir_named(
    zip: &mut ZipWriter<fs::File>,
    input_path: &Path,
    prefix: &str,
    zip_base: &str,
    back_list: &[&str],
) -> Result<(), Box<dyn Error>> {
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut buffer = vec![0; 1024 * 512];
    for entry in input_path.read_dir()? {
        if let Ok(entry) = entry {
            let path = entry.path();
            let relative = path.strip_prefix(Path::new(prefix))?;
            let relative = relative.to_string_lossy().to_string();
            if relative.is_empty() {
                continue;
            }
            if back_list.iter().any(|&x| x == relative) {
                continue;
            }
            let name = if zip_base.is_empty() {
                relative
            } else {
                format!("{}/{}", zip_base, relative)
            };
            Loading::notify_desc(entry.file_name().to_string_lossy().to_string());
            if path.is_file() {
                zip.start_file(&name, options)?;
                let mut input_file = fs::File::open(path)?;
                loop {
                    let size = input_file.read(&mut buffer)?;
                    if size == 0 {
                        break;
                    }
                    zip.write_all(&buffer[0..size])?;
                }
            } else {
                zip.add_directory(format!("{}/", name), options)?;
                zip_dir_named(zip, path.as_path(), prefix, zip_base, back_list)?;
            }
        }
    }

    Ok(())
}

/// Zip several directories into one archive, each stored under its own folder
/// name. `sources` holds `(zip_folder_name, fs_path)` pairs; an empty folder
/// name stores that directory's contents at the archive root.
pub fn zip_dirs(
    sources: &[(String, String)],
    to: &str,
    back_list: &[&str],
) -> Result<(), Box<dyn Error>> {
    let output_path = Path::new(to);
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut zip = zip::ZipWriter::new(fs::File::create(output_path)?);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (folder_name, dir_path) in sources {
        if !Path::new(dir_path).is_dir() {
            continue;
        }
        let root = if dir_path.ends_with("/") {
            dir_path.to_string()
        } else {
            format!("{}/", dir_path)
        };
        if !folder_name.is_empty() {
            zip.add_directory(format!("{}/", folder_name), options)?;
        }
        zip_dir_named(&mut zip, Path::new(&root), &root, folder_name, back_list)?;
    }
    zip.finish()?;
    Ok(())
}

pub fn zip_file(from: &str, name: &str, to: &str) -> Result<(), Box<dyn Error>> {
    let from_path = Path::new(from).join(name);
    let mut zip = zip::ZipWriter::new(fs::File::create(to)?);
    let options =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut buffer = vec![0; 1024 * 512];
    #[allow(deprecated)]
    zip.start_file_from_path(Path::new(name), options)?;
    let mut input_file = fs::File::open(from_path)?;
    loop {
        let size = input_file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        zip.write_all(&buffer[0..size])?;
    }
    zip.finish()?;
    Ok(())
}

pub fn zip_extract(
    from: impl AsRef<Path>,
    to: impl AsRef<Path>,
    back_list: Option<&[&str]>,
) -> Result<(), Box<dyn Error>> {
    let mut zip = zip::ZipArchive::new(fs::File::open(from)?)?;
    for i in 0..zip.len() {
        Loading::notify_title(format!("Extracting {}/{}", i + 1, zip.len()));
        let mut file_name = zip.by_index(i)?;
        let output_path = match file_name.enclosed_name() {
            Some(file_name) => {
                Loading::notify_desc(file_name.to_string_lossy().to_string());
                to.as_ref().join(file_name).to_owned()
            }
            None => continue,
        };

        if (*file_name.name()).ends_with('/') {
            if !output_path.exists() {
                fs::create_dir_all(&output_path)?;
            }
        } else {
            if let Some(p) = output_path.parent() {
                if !p.exists() {
                    fs::create_dir_all(p)?;
                }
            }
            if back_list.is_some_and(|list| list.iter().find(|&&x| x == file_name.name()).is_some())
                && output_path.exists()
            {
                continue;
            }
            let mut output_file = fs::File::create(&output_path)?;
            io::copy(&mut file_name, &mut output_file)?;
        }
    }

    Ok(())
}

pub fn copy_dir_all(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> io::Result<u64> {
    fs::create_dir_all(&dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_all(entry.path(), dst.as_ref().join(entry.file_name()))?;
        } else {
            fs::copy(entry.path(), dst.as_ref().join(entry.file_name()))?;
        }
    }
    Ok(0)
}

pub fn join_path(base: &str, path: &str) -> String {
    if base == "" || base.ends_with("/") {
        format!("{}{}", base, path)
    } else {
        format!("{}/{}", base, path)
    }
}

pub fn update_sfo_file_with_current_account_id(sfo_path: &str) -> Result<(), Box<dyn Error>> {
    if Path::new(&sfo_path).exists() {
        let account_id = get_psv_account_id();
        if account_id > 0 {
            if change_psv_account_id(&sfo_path, account_id) < 0 {
                let msg = format!("change psv account id failed: {}", account_id);
                error!("{}", msg);
                return Err(msg.into());
            }
        } else {
            error!("get psv account id failed");
            return Err("get psv account id failed".into());
        }
    }

    Ok(())
}

pub fn backup_game_save(from: &str, to: &str) -> Result<(), Box<dyn Error>> {
    zip_dir(from, to, &BACKUP_BLACK_LIST)
}

/// What a save is made of: the directories to archive and where a restore puts
/// them back. Native saves live in one directory and use it for both, while a
/// PSP game can own several sibling folders under a shared parent.
#[derive(Debug, Clone)]
pub struct SaveTarget {
    /// `(zip_folder_name, fs_path)` pairs. An empty folder name stores that
    /// directory's contents at the archive root.
    pub sources: Vec<(String, String)>,
    /// Directory a restore extracts into.
    pub restore_root: String,
}

impl SaveTarget {
    /// A save held in a single directory, archived at the root of the zip.
    pub fn single(path: &str) -> SaveTarget {
        SaveTarget {
            sources: vec![(String::new(), path.to_string())],
            restore_root: path.to_string(),
        }
    }

    /// A save spread over sibling folders below `restore_root`. Each folder is
    /// archived under its own name so the zip stays rooted at the parent and
    /// extracts back into place.
    pub fn grouped(paths: &[String], restore_root: &str) -> SaveTarget {
        let mut sources: Vec<(String, String)> = paths
            .iter()
            .map(|path| {
                let name = Path::new(path)
                    .file_name()
                    .unwrap_or(OsStr::new(""))
                    .to_string_lossy()
                    .to_string();
                (name, path.to_string())
            })
            .collect();
        // Sorted so the plain title-id folder comes before its suffixed
        // siblings, making it the primary. Scanning prefers the PROFILE folder
        // for the icon, which is the wrong pick to fall back to on restore.
        sources.sort();
        SaveTarget {
            sources,
            restore_root: restore_root.to_string(),
        }
    }

    /// Primary source directory, used when an archive turns out to be rooted
    /// inside the save folder rather than at its parent.
    pub fn primary_source(&self) -> &str {
        self.sources
            .first()
            .map(|(_, path)| path.as_str())
            .unwrap_or(&self.restore_root)
    }

    fn is_rooted_at_contents(&self) -> bool {
        self.sources.len() == 1 && self.sources[0].0.is_empty()
    }
}

/// True when the archive's top level holds save folders, meaning it belongs in
/// the shared parent directory. False for archives rooted inside a single game
/// folder, which some desktop client versions produce.
pub fn is_grouped_archive(from: &str) -> bool {
    let file = match fs::File::open(from) {
        Ok(file) => file,
        Err(_) => return true,
    };
    let mut zip = match zip::ZipArchive::new(file) {
        Ok(zip) => zip,
        Err(_) => return true,
    };
    for i in 0..zip.len() {
        let name = match zip.by_index(i) {
            Ok(entry) => entry.name().to_string(),
            Err(_) => continue,
        };
        // Only a name with a separator proves the top segment is a directory.
        if !name.contains('/') {
            continue;
        }
        if let Some(top) = name.split('/').next() {
            if psp_title_prefix(top).is_some() {
                return true;
            }
        }
    }
    false
}

/// Directory an archive should be extracted into. A grouped target normally
/// restores to its shared parent, but a game-rooted archive has to go straight
/// into the save folder instead.
pub fn restore_root_for(target: &SaveTarget, from: &str) -> String {
    if target.is_rooted_at_contents() || is_grouped_archive(from) {
        target.restore_root.to_string()
    } else {
        target.primary_source().to_string()
    }
}

pub fn backup_save_target(target: &SaveTarget, to: &str) -> Result<(), Box<dyn Error>> {
    if target.is_rooted_at_contents() {
        zip_dir(&target.sources[0].1, to, &BACKUP_BLACK_LIST)
    } else {
        zip_dirs(&target.sources, to, &BACKUP_BLACK_LIST)
    }
}

/// Top-level directory names an archive will write into.
fn archive_top_level_dirs(from: &str) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    let file = match fs::File::open(from) {
        Ok(file) => file,
        Err(_) => return dirs,
    };
    let mut zip = match zip::ZipArchive::new(file) {
        Ok(zip) => zip,
        Err(_) => return dirs,
    };
    for i in 0..zip.len() {
        let name = match zip.by_index(i) {
            Ok(entry) => entry.name().to_string(),
            Err(_) => continue,
        };
        if !name.contains('/') {
            continue;
        }
        if let Some(top) = name.split('/').next() {
            if !top.is_empty() && !dirs.iter().any(|d| d == top) {
                dirs.push(top.to_string());
            }
        }
    }
    dirs
}

/// What the auto-backup has to cover: the save itself, plus any other folder
/// the archive is about to overwrite. Archives written before saves were
/// scoped per game hold every game, and losing the bystanders is not
/// recoverable otherwise.
fn overwrite_scope(target: &SaveTarget, from: &str, to: &str) -> SaveTarget {
    let mut sources = target.sources.clone();
    if to == target.restore_root {
        for dir in archive_top_level_dirs(from) {
            if sources.iter().any(|(name, _)| name == &dir) {
                continue;
            }
            let path = format!("{}/{}", target.restore_root, dir);
            if Path::new(&path).is_dir() {
                sources.push((dir, path));
            }
        }
    }
    SaveTarget {
        sources,
        restore_root: target.restore_root.to_string(),
    }
}

pub fn restore_save_target(target: &SaveTarget, from: &str) -> Result<(), Box<dyn Error>> {
    let to = restore_root_for(target, from);
    if let Some(from_parent) = Path::new(from).parent() {
        if let Some(auto_backup_path) = from_parent
            .join(&format!("{} auto.zip", get_current_format_time()))
            .to_str()
        {
            Loading::notify_title("Auto-backing up...".to_string());
            let _ = backup_save_target(&overwrite_scope(target, from, &to), auto_backup_path);
        }
    }
    Loading::notify_title("Restoring save...".to_string());
    let mut res = zip_extract(from, &to, Some(&BACKUP_BLACK_LIST));
    if res.is_ok() {
        let sfo_path = format!("{}/sce_sys/param.sfo", to);
        res = update_sfo_file_with_current_account_id(&sfo_path);
    }
    res
}

pub fn base64_encode(data: &[u8]) -> String {
    general_purpose::STANDARD.encode(data)
}

pub fn base64_decode(data: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    Ok(general_purpose::STANDARD.decode(data)?)
}

pub fn get_str_md5(data: &[u8]) -> String {
    format!("{:x}", md5::compute(data))
}

pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(data);
    let hex: String = hash.iter().map(|b| format!("{:02x}", b)).collect();
    format!("sha256:{}", hex)
}

pub fn sha256_file(path: &str) -> Result<String, Box<dyn Error>> {
    let data = fs::read(path)?;
    Ok(sha256_hex(&data))
}

pub fn delete_dir_if_empty(path: &str) -> Result<(), Box<dyn Error>> {
    let path = Path::new(path);
    if path.exists() && path.is_dir() && path.read_dir()?.next().is_none() {
        fs::remove_dir(path)?;
    }
    Ok(())
}

pub fn get_game_local_backup_dir(title_id: &str, name: &str) -> String {
    let default_dir_path = format!(
        "{}/{} {}",
        GAME_SAVE_LOCAL_DIR,
        title_id,
        normalize_path(name)
    )
    .trim()
    .to_string();

    let path = Path::new(GAME_SAVE_LOCAL_DIR);
    if !path.exists() {
        return default_dir_path;
    }

    if let Ok(dir_iter) = path.read_dir() {
        for entry in dir_iter {
            if let Ok(entry) = entry {
                let path = entry.path();
                if path.is_dir() {
                    let name = path
                        .file_name()
                        .unwrap_or(OsStr::new(""))
                        .to_str()
                        .unwrap_or("");
                    if !name.is_empty() && name.starts_with(title_id) {
                        return path.to_str().unwrap_or(&default_dir_path).to_string();
                    }
                }
            }
        }
    }

    default_dir_path
}

pub fn create_parent_if_not_exists(path: &str) -> Result<(), Box<dyn Error>> {
    match Path::new(path).parent() {
        Some(parent) => {
            if !parent.exists() {
                fs::create_dir_all(parent)?;
            }
        }
        None => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::utils::{base64_decode, base64_encode};

    use super::ease_out_expo;

    #[test]
    pub fn test_md5() {
        let mut context = md5::Context::new();
        context.consume("123");
        context.consume("456");
        let data = "123456";
        let res = md5::compute(data);
        assert_eq!("e10adc3949ba59abbe56e057f20f883e", format!("{:x}", res));
        assert_eq!(
            "e10adc3949ba59abbe56e057f20f883e",
            format!("{:x}", context.compute())
        );
    }

    #[test]
    pub fn test_base64() -> Result<(), Box<dyn std::error::Error>> {
        let data = String::from("hello world");
        let data = data.as_bytes();
        let key = base64_encode(data);
        let res = String::from_utf8_lossy(&base64_decode(&key)?).to_string();
        assert_eq!(res, "hello world");

        Ok(())
    }

    #[test]
    pub fn test_ease_out_expo() {
        assert_eq!(
            ease_out_expo(
                Duration::from_millis(1),
                Duration::from_millis(10),
                0.0,
                10.0,
            ),
            5.0
        );

        assert_eq!(
            ease_out_expo(
                Duration::from_millis(2),
                Duration::from_millis(10),
                0.0,
                10.0,
            ),
            7.5
        );
        assert_eq!(
            ease_out_expo(
                Duration::from_millis(3),
                Duration::from_millis(10),
                0.0,
                10.0,
            ),
            8.75
        );
        assert_eq!(
            ease_out_expo(
                Duration::from_millis(10),
                Duration::from_millis(10),
                0.0,
                10.0,
            ),
            10.0
        );
    }

    #[test]
    fn test_normalize_path() {
        let path = "a\\a/a:a*a?a\"a\'a<a>a|a";
        let path = super::normalize_path(path);
        assert_eq!(
            "a_a_a_a_a_a_a_a_a_a_a",
            path
        );
    }
}
