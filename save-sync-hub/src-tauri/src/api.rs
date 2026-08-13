use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::config::SyncConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudManifest {
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub games: HashMap<String, CloudGameEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudGameEntry {
    #[serde(rename = "latestVersion")]
    pub latest_version: String,
    #[serde(rename = "latestHash")]
    pub latest_hash: String,
    #[serde(rename = "uploadedBy")]
    pub uploaded_by: String,
    pub size: u64,
    #[serde(rename = "versionCount", default)]
    pub version_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEntry {
    #[serde(rename = "deviceId")]
    pub device_id: String,
    #[serde(rename = "pairedAt")]
    pub paired_at: String,
}

fn build_url(config: &SyncConfig, path: &str) -> String {
    format!("{}{}", config.server_url.trim_end_matches('/'), path)
}

/// Encode a save identifier for use as a URL path segment. Ids are derived
/// from filesystem names (RetroArch ROM names), so they can contain spaces
/// and other URL-hostile bytes. Fastify decodes the segment back on the
/// server.
fn url_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

fn auth_value(config: &SyncConfig) -> String {
    format!("Bearer {}", config.api_token)
}

pub fn get_manifest(config: &SyncConfig) -> Result<CloudManifest, String> {
    let url = build_url(config, "/api/manifest");
    let response = ureq::get(&url)
        .set("Authorization", &auth_value(config))
        .call()
        .map_err(|e| format!("Request failed: {}", e))?;
    let body = response
        .into_string()
        .map_err(|e| format!("Read response failed: {}", e))?;
    serde_json::from_str(&body).map_err(|e| format!("Parse manifest failed: {}", e))
}

pub fn upload_save(
    config: &SyncConfig,
    title_id: &str,
    content_hash: &str,
    zip_path: &str,
    hash: &str,
    timestamp: &str,
) -> Result<(), String> {
    let url = build_url(config, &format!("/api/save/{}", url_segment(title_id)));
    let file_data =
        std::fs::read(zip_path).map_err(|e| format!("Read file failed: {}", e))?;

    let boundary = format!("SaveSyncHub{}", std::process::id());
    let mut body = Vec::new();
    let filename = std::path::Path::new(zip_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{}.zip", title_id));

    let part_header = format!(
        "--{}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: application/zip\r\n\r\n",
        boundary, filename
    );
    body.extend_from_slice(part_header.as_bytes());
    body.extend_from_slice(&file_data);
    body.extend_from_slice(format!("\r\n--{}--\r\n", boundary).as_bytes());

    let mut request = ureq::put(&url)
        .set("Authorization", &auth_value(config))
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={}", boundary),
        )
        .set("X-Device-Id", &config.device_name)
        .set("X-Save-Hash", hash)
        .set("X-Save-Timestamp", timestamp);
    // Canonical content hash of the save files so status checks can compare
    // saves across devices (matches the Vita app's algorithm).
    if !content_hash.is_empty() {
        request = request.set("X-Content-Hash", content_hash);
    }

    let response = request.send_bytes(&body).map_err(|e| format!("Upload failed: {}", e))?;

    if response.status() != 200 {
        let body = response
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        return Err(format!("Upload failed: {}", body));
    }
    Ok(())
}

pub fn download_save(config: &SyncConfig, title_id: &str, dest_path: &str) -> Result<(), String> {
    let url = build_url(config, &format!("/api/save/{}", url_segment(title_id)));
    let response = ureq::get(&url)
        .set("Authorization", &auth_value(config))
        .call()
        .map_err(|e| format!("Download failed: {}", e))?;

    let status = response.status();
    if status == 404 {
        return Err("No save on server for this game".to_string());
    }
    if status != 200 {
        let body = response
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        return Err(format!("Download failed ({}): {}", status, body));
    }

    let mut reader = response.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf)
        .map_err(|e| format!("Read body failed: {}", e))?;

    if let Some(parent) = std::path::Path::new(dest_path).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Create dir failed: {}", e))?;
    }
    std::fs::write(dest_path, &buf).map_err(|e| format!("Write file failed: {}", e))?;
    Ok(())
}

pub fn delete_save(config: &SyncConfig, title_id: &str) -> Result<(), String> {
    let url = build_url(config, &format!("/api/save/{}", url_segment(title_id)));
    let response = ureq::delete(&url)
        .set("Authorization", &auth_value(config))
        .call()
        .map_err(|e| format!("Delete failed: {}", e))?;

    if response.status() != 200 {
        let body = response
            .into_string()
            .unwrap_or_else(|_| "unknown".to_string());
        return Err(format!("Delete failed: {}", body));
    }
    Ok(())
}

pub fn get_devices(config: &SyncConfig) -> Result<Vec<DeviceEntry>, String> {
    let url = build_url(config, "/api/devices");
    let response = ureq::get(&url)
        .set("Authorization", &auth_value(config))
        .call()
        .map_err(|e| format!("Request failed: {}", e))?;
    let body = response
        .into_string()
        .map_err(|e| format!("Read response failed: {}", e))?;
    serde_json::from_str(&body).map_err(|e| format!("Parse devices failed: {}", e))
}
