use sha2::{Digest, Sha256};
use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use zip::ZipWriter;

pub fn sha256_hex(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("sha256:{}", hash.iter().map(|b| format!("{:02x}", b)).collect::<String>())
}

pub fn sha256_file(path: &str) -> Result<String, String> {
    let data = fs::read(path).map_err(|e| format!("Read file failed: {}", e))?;
    Ok(sha256_hex(&data))
}

pub fn zip_dir(from: &str, to: &str) -> Result<(), String> {
    let from = if from.ends_with('/') {
        from.to_string()
    } else {
        format!("{}/", from)
    };
    let output_path = Path::new(to);
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| format!("Create dir failed: {}", e))?;
        }
    }
    let file = fs::File::create(output_path).map_err(|e| format!("Create zip failed: {}", e))?;
    let mut zip = ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    zip_inner(&mut zip, Path::new(&from), &from, &options)?;
    zip.finish().map_err(|e| format!("Finish zip failed: {}", e))?;
    Ok(())
}

fn zip_inner(
    zip: &mut ZipWriter<fs::File>,
    dir: &Path,
    prefix: &str,
    options: &zip::write::FileOptions,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 1024 * 512];
    for entry in dir.read_dir().map_err(|e| format!("Read dir failed: {}", e))? {
        let entry = entry.map_err(|e| format!("Dir entry failed: {}", e))?;
        let path = entry.path();
        let name = path
            .strip_prefix(Path::new(prefix))
            .map_err(|e| format!("Strip prefix failed: {}", e))?;
        if path.is_file() {
            zip.start_file(
                name.to_string_lossy(),
                *options,
            )
            .map_err(|e| format!("Zip start file failed: {}", e))?;
            let mut f = fs::File::open(&path).map_err(|e| format!("Open file failed: {}", e))?;
            loop {
                let n = f.read(&mut buffer).map_err(|e| format!("Read failed: {}", e))?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buffer[..n])
                    .map_err(|e| format!("Zip write failed: {}", e))?;
            }
        } else if !name.as_os_str().is_empty() {
            zip.add_directory(
                name.to_string_lossy(),
                *options,
            )
            .map_err(|e| format!("Zip add dir failed: {}", e))?;
            zip_inner(zip, &path, prefix, options)?;
        }
    }
    Ok(())
}

/// Canonical content hash of a save: sha256 over `(relative path, bytes)` of
/// every file, sorted by relative path. Identical to the Vita app's algorithm
/// so saves compare across devices through the server. Returns None when any
/// source is missing.
pub fn content_hash(sources: &[(String, String)]) -> Option<String> {
    use sha2::Digest;

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for (entry_name, path) in sources {
        let p = Path::new(path);
        if p.is_dir() {
            let mut rels: Vec<String> = Vec::new();
            collect_rel_files(p, "", &mut rels).ok()?;
            for rel in rels {
                let bytes = fs::read(format!("{}/{}", path, rel)).ok()?;
                let full = if entry_name.is_empty() {
                    rel
                } else {
                    format!("{}/{}", entry_name, rel)
                };
                files.push((full, bytes));
            }
        } else if p.is_file() {
            if entry_name.is_empty() {
                continue;
            }
            let bytes = fs::read(path).ok()?;
            files.push((entry_name.clone(), bytes));
        }
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = sha2::Sha256::new();
    for (rel, bytes) in &files {
        hasher.update(rel.as_bytes());
        hasher.update(&[0u8]);
        hasher.update(bytes);
    }
    let hash = hasher.finalize();
    Some(format!(
        "sha256:{}",
        hash.iter().map(|b| format!("{:02x}", b)).collect::<String>()
    ))
}

/// Relative paths of every file below `dir`, sorted after collection so the
/// hash never depends on readdir order.
fn collect_rel_files(dir: &Path, prefix: &str, out: &mut Vec<String>) -> std::io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{}/{}", prefix, name)
        };
        if path.is_dir() {
            collect_rel_files(&path, &rel, out)?;
        } else {
            out.push(rel);
        }
    }
    out.sort();
    Ok(())
}

/// Zip multiple sources into one archive. Directories are stored under their
/// folder name; files are stored at their entry name, which may be a relative
/// path so a restore puts them back into a subfolder.
/// `sources` is a slice of `(zip_entry_name, fs_path)` pairs.
pub fn zip_dirs(sources: &[(String, String)], to: &str) -> Result<(), String> {
    let output_path = Path::new(to);
    if let Some(parent) = output_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| format!("Create dir failed: {}", e))?;
        }
    }
    let file = fs::File::create(output_path).map_err(|e| format!("Create zip failed: {}", e))?;
    let mut zip = ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let mut buffer = vec![0u8; 1024 * 512];
    for (entry_name, source_path) in sources {
        let path = Path::new(source_path);
        if path.is_file() {
            if entry_name.is_empty() {
                continue;
            }
            zip.start_file(entry_name, options)
                .map_err(|e| format!("Zip start file failed: {}", e))?;
            let mut f = fs::File::open(path).map_err(|e| format!("Open file failed: {}", e))?;
            loop {
                let n = f.read(&mut buffer).map_err(|e| format!("Read failed: {}", e))?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buffer[..n])
                    .map_err(|e| format!("Zip write failed: {}", e))?;
            }
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        let root = if source_path.ends_with('/') {
            source_path.clone()
        } else {
            format!("{}/", source_path)
        };
        if !entry_name.is_empty() {
            zip.add_directory(format!("{}/", entry_name), options)
                .map_err(|e| format!("Zip add dir failed: {}", e))?;
        }
        zip_inner_named(&mut zip, Path::new(&root), &root, entry_name, &options)?;
    }

    zip.finish().map_err(|e| format!("Finish zip failed: {}", e))?;
    Ok(())
}

fn zip_inner_named(
    zip: &mut ZipWriter<fs::File>,
    dir: &Path,
    root: &str,
    zip_base: &str,
    options: &zip::write::FileOptions,
) -> Result<(), String> {
    let mut buffer = vec![0u8; 1024 * 512];
    for entry in dir.read_dir().map_err(|e| format!("Read dir failed: {}", e))? {
        let entry = entry.map_err(|e| format!("Dir entry failed: {}", e))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(Path::new(root))
            .map_err(|e| format!("Strip prefix failed: {}", e))?;
        let entry_name = format!("{}/{}", zip_base, relative.to_string_lossy());

        if path.is_file() {
            zip.start_file(&entry_name, *options)
                .map_err(|e| format!("Zip start file failed: {}", e))?;
            let mut f = fs::File::open(&path).map_err(|e| format!("Open file failed: {}", e))?;
            loop {
                let n = f.read(&mut buffer).map_err(|e| format!("Read failed: {}", e))?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buffer[..n])
                    .map_err(|e| format!("Zip write failed: {}", e))?;
            }
        } else {
            zip.add_directory(format!("{}/", entry_name), *options)
                .map_err(|e| format!("Zip add dir failed: {}", e))?;
            zip_inner_named(zip, &path, root, zip_base, options)?;
        }
    }
    Ok(())
}

pub fn zip_extract(from: &str, to: &str) -> Result<(), String> {
    let file = fs::File::open(from).map_err(|e| format!("Open zip failed: {}", e))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("Read zip failed: {}", e))?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Zip entry failed: {}", e))?;
        let out_path = match entry.enclosed_name() {
            Some(name) => Path::new(to).join(name),
            None => continue,
        };
        if entry.name().ends_with('/') {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Create dir failed: {}", e))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Create parent dir failed: {}", e))?;
            }
            let mut out =
                fs::File::create(&out_path).map_err(|e| format!("Create file failed: {}", e))?;
            io::copy(&mut entry, &mut out)
                .map_err(|e| format!("Extract failed: {}", e))?;
        }
    }
    Ok(())
}
