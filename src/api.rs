use std::collections::HashMap;
use std::fs;
use std::path::Path;

use log::error;
use serde::{Deserialize, Serialize};

use crate::config::Config;

#[derive(Debug, Deserialize)]
pub struct StatusResponse {
    pub ok: bool,
    #[serde(rename = "serverVersion")]
    pub server_version: String,
    pub features: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PairResponse {
    pub ok: bool,
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "deviceId")]
    pub device_id: String,
}

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
}

#[derive(Debug, Deserialize)]
pub struct UploadResponse {
    pub ok: bool,
    #[serde(rename = "titleId")]
    pub title_id: String,
    pub version: String,
}

pub struct Api;

impl Api {
    fn build_url(config: &Config, path: &str) -> String {
        format!("{}{}", config.server_url.trim_end_matches('/'), path)
    }

    fn auth_value(config: &Config) -> String {
        format!("Bearer {}", config.api_token)
    }

    pub fn test_connection(config: &Config) -> Result<StatusResponse, String> {
        // Step 1: check reachability and get server version
        let url = Api::build_url(config, "/api/status");
        let response = ureq::get(&url)
            .call()
            .map_err(|e| format!("Connection failed: {}", e))?;
        let body = response
            .into_string()
            .map_err(|e| format!("Read failed: {}", e))?;
        let status: StatusResponse =
            serde_json::from_str(&body).map_err(|e| format!("Invalid response: {}", e))?;

        // Step 2: validate token against an auth-protected endpoint
        let manifest_url = Api::build_url(config, "/api/manifest");
        let auth_resp = ureq::get(&manifest_url)
            .set("Authorization", &Api::auth_value(config))
            .call();
        match auth_resp {
            Err(ureq::Error::Status(401, _)) => {
                return Err("Invalid token. Check Settings.".to_string());
            }
            Err(e) => {
                return Err(format!("Token check failed: {}", e));
            }
            Ok(_) => {}
        }

        Ok(status)
    }

    pub fn get_cloud_manifest(config: &Config) -> Result<CloudManifest, String> {
        let url = Api::build_url(config, "/api/manifest");
        let response = ureq::get(&url)
            .set("Authorization", &Api::auth_value(config))
            .call()
            .map_err(|e| format!("Connection failed: {}", e))?;
        let status = response.status();
        let body = response
            .into_string()
            .map_err(|e| format!("Read failed: {}", e))?;
        if status == 401 {
            return Err("Invalid token. Check Settings.".to_string());
        }
        if status != 200 {
            return Err(format!("Server error {}: {}", status, body));
        }
        serde_json::from_str(&body).map_err(|e| format!("Invalid response: {}", e))
    }

    pub fn upload_save(
        config: &Config,
        title_id: &str,
        zip_path: &str,
        hash: &str,
        timestamp: &str,
    ) -> Result<UploadResponse, String> {
        let url = Api::build_url(config, &format!("/api/save/{}", title_id));
        let file_data = fs::read(zip_path).map_err(|e| format!("Read file failed: {}", e))?;

        let boundary = format!("VitaSaveSync{}", std::process::id());
        let mut body = Vec::new();

        // Build multipart body manually (avoids ureq_multipart compat issues)
        let filename = Path::new(zip_path)
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

        let response = ureq::put(&url)
            .set("Authorization", &Api::auth_value(config))
            .set("Content-Type", &format!("multipart/form-data; boundary={}", boundary))
            .set("X-Device-Id", &config.device_name)
            .set("X-Save-Hash", hash)
            .set("X-Save-Timestamp", timestamp)
            .send_bytes(&body)
            .map_err(|e| format!("Upload failed: {}", e))?;

        let status = response.status();
        let resp_body = response
            .into_string()
            .map_err(|e| format!("Read response failed: {}", e))?;

        if status != 200 {
            error!("Upload returned {}: {}", status, resp_body);
            return Err(format!("Upload failed: {}", resp_body));
        }
        serde_json::from_str(&resp_body).map_err(|e| format!("Invalid response: {}", e))
    }

    pub fn download_save(config: &Config, title_id: &str, dest_path: &str) -> Result<(), String> {
        let url = Api::build_url(config, &format!("/api/save/{}", title_id));
        let response = ureq::get(&url)
            .set("Authorization", &Api::auth_value(config))
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

        // Read response body into vec, then write to file
        let mut reader = response.into_reader();
        let mut buf = Vec::new();
        use std::io::Read;
        reader
            .read_to_end(&mut buf)
            .map_err(|e| format!("Read body failed: {}", e))?;

        if let Some(parent) = Path::new(dest_path).parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Create dir failed: {}", e))?;
        }
        fs::write(dest_path, &buf).map_err(|e| format!("Write file failed: {}", e))?;

        Ok(())
    }
}
