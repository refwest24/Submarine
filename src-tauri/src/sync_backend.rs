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

/// Where the S3 transport syncs to. No secrets: the keys stay in the s3cmd
/// config file named here and are read only when a sync runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct S3Settings {
    /// s3cmd-format config holding `access_key` / `secret_key`; `~/` allowed.
    pub credentials_file: String,
    pub bucket: String,
    /// Key prefix inside the bucket, so one bucket can serve other purposes.
    pub prefix: String,
    /// Overrides the credentials file's `host_base`, e.g. `http://127.0.0.1:9000`.
    pub endpoint: Option<String>,
    /// Overrides the region derived from `bucket_location`.
    pub region: Option<String>,
    /// `https://host/bucket/key` (works on every S3-compatible service) rather
    /// than `https://bucket.host/key`.
    pub path_style: bool,
}

impl Default for S3Settings {
    fn default() -> Self {
        Self {
            credentials_file: "~/.config/submarine-sync/s3cfg".into(),
            bucket: String::new(),
            prefix: "submarine/sync".into(),
            endpoint: None,
            region: None,
            path_style: true,
        }
    }
}

/// Per-install sync settings. Holds no secrets.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BackendConfig {
    pub backend: Backend,
    pub s3: S3Settings,
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

/// Write via temp file + rename so a crash can't leave a half-written config
/// (which `load_config` would then refuse).
fn save_config(app: &tauri::AppHandle, cfg: &BackendConfig) -> Result<(), String> {
    let path = config_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("[S3] CONFIG_WRITE: {e}"))?;
    }
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| format!("[S3] CONFIG_WRITE: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes).map_err(|e| format!("[S3] CONFIG_WRITE: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("[S3] CONFIG_WRITE: {e}"))
}

/// Normalise and check S3 settings before they're saved or tried. The bucket
/// and prefix end up in request paths (and, virtual-host style, in a host
/// name), so they're held to plain characters.
pub fn validate_s3(s: &S3Settings) -> Result<S3Settings, String> {
    let bad = |what: &str| Err(format!("[S3] NOT_CONFIGURED: {what}"));
    let mut s = s.clone();
    s.credentials_file = s.credentials_file.trim().to_string();
    s.bucket = s.bucket.trim().to_string();
    s.prefix = s.prefix.trim().trim_matches('/').to_string();
    s.endpoint = s.endpoint.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
    s.region = s.region.map(|r| r.trim().to_string()).filter(|r| !r.is_empty());
    if s.credentials_file.is_empty() {
        return bad("choose the s3cmd config file that holds the access key");
    }
    if s.bucket.is_empty()
        || s.bucket.len() > 255
        || !s.bucket.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'_')
    {
        return bad("the bucket name may only use letters, digits, '.', '-' and '_'");
    }
    let prefix_ok = s.prefix.len() <= 200
        && s.prefix.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
        && (s.prefix.is_empty() || s.prefix.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != ".."));
    if !prefix_ok {
        return bad("the prefix may only use letters, digits, '.', '-', '_' and single '/' between parts");
    }
    if let Some(e) = &s.endpoint {
        let ok = url::Url::parse(e).is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
        if !ok {
            return bad("the endpoint must be an http(s) URL, e.g. https://us-ord-10.linodeobjects.com");
        }
    }
    if let Some(r) = &s.region {
        if r.len() > 64 || !r.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return bad("the region may only use letters, digits, '-' and '_'");
        }
    }
    Ok(s)
}

/// Whether personal sync can run on this install, without a network call:
/// HTTP needs a cloud session; S3 needs a bucket and a readable credentials
/// file. An unreadable config counts as S3-not-ready (only S3 setups write it).
pub async fn personal_sync_status(app: &tauri::AppHandle, cloud: &CloudState) -> (Backend, bool) {
    match load_config(app) {
        Ok(cfg) if cfg.backend == Backend::Http => (Backend::Http, cloud.status().await.signed_in),
        Ok(cfg) => (
            Backend::S3,
            !cfg.s3.bucket.trim().is_empty() && crate::s3_store::read_s3cmd_config(&cfg.s3.credentials_file).is_ok(),
        ),
        Err(_) => (Backend::S3, false),
    }
}

/// What the settings screen shows. Never includes the keys.
#[derive(Serialize)]
pub struct SyncBackendView {
    backend: Backend,
    s3: S3Settings,
    /// The credentials file exists and has an access key + secret.
    credentials_ok: bool,
    credentials_error: Option<String>,
    /// Endpoint/region the S3 transport would start with (settings first,
    /// then the credentials file), when the file is readable.
    endpoint: Option<String>,
    region: Option<String>,
}

fn view(cfg: BackendConfig) -> SyncBackendView {
    let creds = crate::s3_store::read_s3cmd_config(&cfg.s3.credentials_file);
    let effective = creds.as_ref().ok().and_then(|c| crate::s3_store::endpoint_and_region(&cfg.s3, c).ok());
    SyncBackendView {
        backend: cfg.backend,
        credentials_ok: creds.is_ok(),
        credentials_error: creds.err(),
        endpoint: effective.as_ref().map(|(e, _)| e.clone()),
        region: effective.map(|(_, r)| r),
        s3: cfg.s3,
    }
}

#[tauri::command]
pub async fn sync_backend_get(app: tauri::AppHandle) -> Result<SyncBackendView, String> {
    Ok(view(load_config(&app)?))
}

/// Save the per-install sync backend. S3 settings are validated only when S3
/// is selected, so switching back to HTTP always works.
#[tauri::command]
pub async fn sync_backend_set(app: tauri::AppHandle, config: BackendConfig) -> Result<SyncBackendView, String> {
    let cfg = match config.backend {
        Backend::S3 => BackendConfig { backend: Backend::S3, s3: validate_s3(&config.s3)? },
        Backend::Http => config,
    };
    save_config(&app, &cfg)?;
    Ok(view(cfg))
}

/// Try `settings` for real before they're saved: write, read, list and
/// delete one small probe object under the prefix. The `.probe-` name can
/// never be taken for a profile partition.
#[tauri::command]
pub async fn s3_test_connection(settings: S3Settings) -> Result<String, String> {
    use crate::object_sync::ObjectStore as _;
    let settings = validate_s3(&settings)?;
    let store = crate::s3_store::S3Store::new(&settings)?;
    let base = if settings.prefix.is_empty() { String::new() } else { format!("{}/", settings.prefix) };
    let probe = format!("{base}.probe-{}", crate::new_entity_uuid());
    store.put(&probe, b"submarine connection test".to_vec()).await?;
    let read = store.get(&probe).await?;
    let listed = store.list(&probe).await?;
    store.delete(&probe).await?;
    if read.is_none() || listed.is_empty() {
        return Err("[S3] BAD_RESPONSE: wrote a test object but couldn't read it back".into());
    }
    let (endpoint, region) = store.describe();
    Ok(format!(
        "Connected to bucket '{}' at {endpoint} (region {region}) — write, read, list and delete all work.",
        settings.bucket
    ))
}

/// The transport personal-profile sync should use on this install.
pub fn personal_transport(
    app: &tauri::AppHandle,
    cloud: &Arc<CloudState>,
) -> Result<Box<dyn SyncTransport>, String> {
    let cfg = load_config(app)?;
    match cfg.backend {
        Backend::Http => Ok(Box::new(HttpTransport { app: app.clone(), cloud: Arc::clone(cloud) })),
        Backend::S3 => s3_transport(&cfg.s3),
    }
}

pub fn s3_transport(settings: &S3Settings) -> Result<Box<dyn SyncTransport>, String> {
    let store = crate::s3_store::S3Store::new(settings)?;
    Ok(Box::new(crate::object_sync::ObjectSync::new(Arc::new(store), &settings.prefix)))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s3(bucket: &str, prefix: &str) -> S3Settings {
        S3Settings { bucket: bucket.into(), prefix: prefix.into(), ..S3Settings::default() }
    }

    #[test]
    fn s3_settings_are_normalised_and_checked() {
        let ok = validate_s3(&S3Settings { endpoint: Some("  ".into()), ..s3(" salamis1 ", "/submarine/sync/") }).unwrap();
        assert_eq!((ok.bucket.as_str(), ok.prefix.as_str(), ok.endpoint.as_deref()), ("salamis1", "submarine/sync", None));
        assert!(validate_s3(&s3("b", "")).is_ok(), "an empty prefix is allowed");
        for bad in [
            s3("", "p"),
            s3("bad bucket", "p"),
            s3("b", "a/../c"),
            s3("b", "a//c"),
            s3("b", "a b"),
            S3Settings { endpoint: Some("ftp://x".into()), ..s3("b", "p") },
            S3Settings { region: Some("us east".into()), ..s3("b", "p") },
            S3Settings { credentials_file: " ".into(), ..s3("b", "p") },
        ] {
            assert!(validate_s3(&bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn a_missing_or_partial_config_file_defaults_to_http() {
        let cfg: BackendConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(cfg.backend, Backend::Http);
        let cfg: BackendConfig = serde_json::from_str(r#"{"backend":"s3","s3":{"bucket":"b"}}"#).unwrap();
        assert_eq!((cfg.backend, cfg.s3.prefix.as_str(), cfg.s3.path_style), (Backend::S3, "submarine/sync", true));
    }

    // Against a real endpoint; skipped unless SUBMARINE_S3_IT=1.
    #[tokio::test]
    async fn it_connection_test_round_trips_a_probe() {
        let Some(settings) = crate::sync_engine_tests::it_settings() else { return };
        let msg = s3_test_connection(settings).await.unwrap();
        assert!(msg.starts_with("Connected"), "{msg}");
    }
}
