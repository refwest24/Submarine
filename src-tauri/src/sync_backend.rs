//! Transport seam for PERSONAL profile sync.
//!
//! The sync engine in lib.rs (collect → exchange → LWW apply) doesn't care where
//! records go; this module picks the transport per install:
//!   - `http` (default): the Submarine cloud server, which LWW-merges records
//!     server-side. Wire traffic is exactly what it was before this seam existed.
//!   - `s3`: any S3-compatible bucket; the merge happens on the client.
//!
//! Shared profiles never come through here — sharing needs the cloud account
//! (members, sealed DEK grants), so they always talk to the HTTP server.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::Manager as _;

use crate::cloud::{self, CloudState, SyncProfileInfo};
use crate::SyncRecord;

const CONFIG_FILENAME: &str = "sync_backend.json";

/// Result of one exchange: the remote records the caller should LWW-apply, and
/// how many records this device actually sent.
pub struct ExchangeOut {
    pub remote: Vec<SyncRecord>,
    pub pushed: usize,
}

#[async_trait]
pub trait SyncTransport: Send + Sync {
    /// Push `push` (this device's full record set, plus the DEK escrow record)
    /// and return every remote record that may be newer than what the caller
    /// holds. The caller applies them with LWW, so returning extra is harmless.
    async fn exchange(&self, profile: &str, push: &[SyncRecord], name: Option<&str>) -> Result<ExchangeOut, String>;
    /// The remote view's (uuid, entity_type, updated_at, deleted) per record,
    /// for the stats diff. Blobs may be omitted.
    async fn index(&self, profile: &str) -> Result<Vec<SyncRecord>, String>;
    /// Every published DEK-escrow blob for `profile` (normally one).
    async fn escrows(&self, profile: &str) -> Result<Vec<String>, String>;
    async fn list_profiles(&self) -> Result<Vec<SyncProfileInfo>, String>;
    /// Remove the whole remote partition; returns how many records went.
    async fn delete_profile(&self, profile: &str) -> Result<i64, String>;
}

// ---------------------------------------------------------------------------
// Configuration (<app_data>/sync_backend.json)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Http,
    S3,
}

/// Per-install sync settings. Holds no secrets.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BackendConfig {
    pub backend: Backend,
}

fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("[SYSTEM] APP_DATA_DIR_NOT_FOUND: {e}"))?;
    Ok(dir.join(CONFIG_FILENAME))
}

/// No file → the HTTP default. A file that exists but can't be read or parsed
/// is an error, not a silent fallback: falling back to HTTP would upload a
/// profile to a server the user deliberately switched away from.
pub fn load_config(app: &tauri::AppHandle) -> Result<BackendConfig, String> {
    let path = config_path(app)?;
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| format!("[S3] CONFIG_INVALID: {CONFIG_FILENAME}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BackendConfig::default()),
        Err(e) => Err(format!("[S3] CONFIG_INVALID: {CONFIG_FILENAME}: {e}")),
    }
}

/// The transport personal-profile sync should use on this install.
pub fn personal_transport(
    app: &tauri::AppHandle,
    cloud: &Arc<CloudState>,
) -> Result<Box<dyn SyncTransport>, String> {
    match load_config(app)?.backend {
        Backend::Http => Ok(Box::new(HttpTransport { app: app.clone(), cloud: Arc::clone(cloud) })),
        Backend::S3 => Err("[S3] NOT_CONFIGURED: S3 sync is not available in this build".into()),
    }
}

// ---------------------------------------------------------------------------
// HTTP: the Submarine cloud server
// ---------------------------------------------------------------------------

pub struct HttpTransport {
    app: tauri::AppHandle,
    cloud: Arc<CloudState>,
}

#[async_trait]
impl SyncTransport for HttpTransport {
    async fn exchange(&self, profile: &str, push: &[SyncRecord], name: Option<&str>) -> Result<ExchangeOut, String> {
        let remote = cloud::sync_exchange(&self.app, &self.cloud, profile, "", push, name).await?;
        Ok(ExchangeOut { remote, pushed: push.len() })
    }

    // The server answers an empty push with its full view and changes nothing,
    // which is all the stats diff and the restore peek have ever used.
    async fn index(&self, profile: &str) -> Result<Vec<SyncRecord>, String> {
        cloud::sync_exchange(&self.app, &self.cloud, profile, "", &[], None).await
    }

    async fn escrows(&self, profile: &str) -> Result<Vec<String>, String> {
        let peek = cloud::sync_exchange(&self.app, &self.cloud, profile, "", &[], None).await?;
        Ok(peek
            .into_iter()
            .filter(|r| r.entity_type == crate::ESCROW_ETYPE && r.uuid == crate::ESCROW_UUID)
            .filter_map(|r| r.blob)
            .collect())
    }

    async fn list_profiles(&self) -> Result<Vec<SyncProfileInfo>, String> {
        cloud::list_sync_profiles(&self.app, &self.cloud).await
    }

    async fn delete_profile(&self, profile: &str) -> Result<i64, String> {
        cloud::delete_sync_profile(&self.app, &self.cloud, profile).await
    }
}
