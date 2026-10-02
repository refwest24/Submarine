//! Personal-profile sync over a plain object store (S3 and compatibles).
//!
//! The HTTP server merges records for us; a bucket can't, so the client does.
//! Layout under `{root}/{profile}/`:
//!
//! ```text
//! r/{entity_type}/{uuid}/{hex(updated_at)}.rec   one immutable object per record
//!                                                version (body = raw blob bytes)
//! r/{entity_type}/{uuid}/{hex(updated_at)}.del   a tombstone (empty body)
//! escrow/{hex(salt)}                             the profile DEK sealed under its
//!                                                password, one per vault salt
//! meta.json                                      {"name": display label}
//! ```
//!
//! Everything the merge needs is in the KEY, so one LIST is the whole index. A
//! record's winner is its newest version (a tombstone beats a record at the same
//! stamp). Writers only ever add objects, and cleanup only removes versions a
//! newer version has superseded, so concurrent devices need no locks or
//! conditional writes: a race can at worst leave a gap that the next sync from
//! any device holding the data fills back in.
//!
//! Superseded versions are kept for 30 days after upload. That includes a
//! device's own LOSING edit: when two devices change the same record offline,
//! the older edit is still uploaded before the winner replaces it locally, so it
//! stays recoverable from the bucket for a month instead of vanishing.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::cloud::SyncProfileInfo;
use crate::sync_backend::{ExchangeOut, SyncTransport};
use crate::SyncRecord;

/// Requests in flight at once.
const CONCURRENCY: usize = 8;
/// Superseded versions are deleted once they have been in the bucket this long.
const RETENTION_SECS: i64 = 30 * 24 * 3600;
/// Refuse absurd listings / bodies from a misbehaving or hostile store.
const MAX_KEYS: usize = 200_000;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ObjectInfo {
    pub key: String,
    /// Upload time, unix seconds; 0 when the store didn't say.
    pub last_modified: i64,
}

/// The five operations the sync needs from a bucket. Keys are full keys.
#[async_trait]
pub trait ObjectStore: Send + Sync + 'static {
    /// Every object under `prefix`, all pages, in key order.
    async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, String>;
    /// The immediate "directories" under `prefix` (S3 CommonPrefixes for
    /// delimiter `/`), as full prefixes ending in `/`.
    async fn list_dirs(&self, prefix: &str) -> Result<Vec<String>, String>;
    /// `None` when the object doesn't exist.
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String>;
    async fn put(&self, key: &str, body: Vec<u8>) -> Result<(), String>;
    /// Deleting a missing object is not an error.
    async fn delete(&self, key: &str) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Key codec
// ---------------------------------------------------------------------------

/// One stored version of one record, parsed from its key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Version {
    pub etype: String,
    pub uuid: String,
    pub stamp: String,
    pub deleted: bool,
    /// Key relative to the partition prefix.
    pub rel: String,
    pub last_modified: i64,
}

impl Version {
    fn rank(&self) -> (&str, bool) {
        (&self.stamp, self.deleted)
    }
}

pub(crate) enum Entry {
    Version(Version),
    Escrow,
    Meta,
}

pub(crate) fn version_rel(etype: &str, uuid: &str, stamp: &str, deleted: bool) -> String {
    format!("r/{etype}/{uuid}/{}.{}", hex::encode(stamp), if deleted { "del" } else { "rec" })
}

fn valid_etype(s: &str) -> bool {
    (1..=24).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
}

fn valid_uuid(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A partition id (`cloud_profile`): a profile name for legacy profiles, 32 hex
/// for new ones. Both fit this; anything else never becomes a bucket path.
pub(crate) fn valid_partition(s: &str) -> bool {
    (1..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn valid_stamp(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic() && b != b'/')
}

/// Lower-case hex of 1..=128 chars — the only shape we ever write.
fn canonical_hex(s: &str) -> bool {
    (1..=128).contains(&s.len()) && s.len() % 2 == 0 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Parse a key relative to the partition. Anything we didn't write — or a key
/// crafted to look odd — is ignored rather than trusted.
pub(crate) fn parse_rel(rel: &str, last_modified: i64) -> Option<Entry> {
    if rel == "meta.json" {
        return Some(Entry::Meta);
    }
    if let Some(salt) = rel.strip_prefix("escrow/") {
        return canonical_hex(salt).then_some(Entry::Escrow);
    }
    let mut parts = rel.split('/');
    let (Some("r"), Some(etype), Some(uuid), Some(file), None) =
        (parts.next(), parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    let (hex_stamp, deleted) = if let Some(h) = file.strip_suffix(".rec") {
        (h, false)
    } else if let Some(h) = file.strip_suffix(".del") {
        (h, true)
    } else {
        return None;
    };
    if !valid_etype(etype) || !valid_uuid(uuid) || !canonical_hex(hex_stamp) {
        return None;
    }
    let stamp = String::from_utf8(hex::decode(hex_stamp).ok()?).ok()?;
    if !valid_stamp(&stamp) {
        return None;
    }
    Some(Entry::Version(Version {
        etype: etype.to_string(),
        uuid: uuid.to_string(),
        stamp,
        deleted,
        rel: rel.to_string(),
        last_modified,
    }))
}

/// One LIST of a partition, parsed.
#[derive(Default)]
pub(crate) struct Index {
    pub versions: Vec<Version>,
    /// Escrow keys relative to the partition.
    pub escrows: Vec<String>,
    pub meta: bool,
    pub newest_upload: i64,
}

pub(crate) fn build_index(objects: &[ObjectInfo], part: &str) -> Index {
    let mut idx = Index::default();
    for o in objects {
        let Some(rel) = o.key.strip_prefix(part) else { continue };
        match parse_rel(rel, o.last_modified) {
            Some(Entry::Version(v)) => idx.versions.push(v),
            Some(Entry::Escrow) => idx.escrows.push(rel.to_string()),
            Some(Entry::Meta) => idx.meta = true,
            None => continue,
        }
        idx.newest_upload = idx.newest_upload.max(o.last_modified);
    }
    idx
}

/// The winning version per uuid.
pub(crate) fn newest(versions: &[Version]) -> HashMap<&str, &Version> {
    let mut map: HashMap<&str, &Version> = HashMap::new();
    for v in versions {
        let e = map.entry(v.uuid.as_str()).or_insert(v);
        if v.rank() > e.rank() {
            *e = v;
        }
    }
    map
}

// ---------------------------------------------------------------------------
// Merge planning (pure)
// ---------------------------------------------------------------------------

pub(crate) struct Plan<'a> {
    /// Local versions to upload, in upload order (see `upload_group`).
    pub puts: Vec<(&'a SyncRecord, String)>,
    /// Remote record winners newer than what this device holds.
    pub fetch: Vec<Version>,
    /// Remote tombstone winners — fully described by their keys, no download.
    pub tombs: Vec<SyncRecord>,
    /// Superseded versions past retention.
    pub gc: Vec<String>,
}

/// Upload order: referents before referrers (ENTITIES order), tombstones last,
/// with a barrier between groups — so no device can list a server whose
/// credential upload failed or hasn't been sent.
fn upload_group(r: &SyncRecord) -> usize {
    if r.deleted {
        return crate::ENTITIES.len() + 1;
    }
    crate::ENTITIES
        .iter()
        .position(|s| s.table == r.entity_type)
        .unwrap_or(crate::ENTITIES.len())
}

/// `local` is this device's full record set (no escrow); `remote` the
/// partition's versions.
pub(crate) fn plan_exchange<'a>(local: &'a [SyncRecord], remote: &[Version], now: i64) -> Plan<'a> {
    // A uuid can be both a live row and an older tombstone locally; only the
    // newest one is this device's current state.
    let mut local_max: HashMap<&str, &SyncRecord> = HashMap::new();
    for r in local {
        let e = local_max.entry(r.uuid.as_str()).or_insert(r);
        if (r.updated_at.as_str(), r.deleted) > (e.updated_at.as_str(), e.deleted) {
            *e = r;
        }
    }
    let present: HashSet<&str> = remote.iter().map(|v| v.rel.as_str()).collect();
    let winners = newest(remote);

    // Upload this device's current version of every record the bucket doesn't
    // already hold — newer ones so others get them, older ones so a losing
    // offline edit is kept (superseded) instead of silently dropped.
    let mut puts: Vec<(&SyncRecord, String)> = local_max
        .values()
        .filter(|r| valid_etype(&r.entity_type) && valid_uuid(&r.uuid) && valid_stamp(&r.updated_at))
        .map(|r| (*r, version_rel(&r.entity_type, &r.uuid, &r.updated_at, r.deleted)))
        .filter(|(_, rel)| !present.contains(rel.as_str()))
        .collect();
    puts.sort_by(|a, b| upload_group(a.0).cmp(&upload_group(b.0)).then_with(|| a.1.cmp(&b.1)));

    let mut fetch = Vec::new();
    let mut tombs = Vec::new();
    for (uuid, w) in &winners {
        let newer = match local_max.get(uuid) {
            Some(l) => w.rank() > (l.updated_at.as_str(), l.deleted),
            None => true,
        };
        if !newer {
            continue;
        }
        if w.deleted {
            tombs.push(SyncRecord {
                uuid: w.uuid.clone(),
                entity_type: w.etype.clone(),
                updated_at: w.stamp.clone(),
                deleted: true,
                blob: None,
            });
        } else {
            fetch.push((*w).clone());
        }
    }

    let gc = remote
        .iter()
        .filter(|v| winners.get(v.uuid.as_str()).is_some_and(|w| v.rank() < w.rank()))
        .filter(|v| v.last_modified > 0 && now - v.last_modified > RETENTION_SECS)
        .map(|v| v.rel.clone())
        .collect();

    Plan { puts, fetch, tombs, gc }
}

fn now_secs() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn rfc3339(secs: i64) -> String {
    if secs <= 0 {
        return String::new();
    }
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|t| t.format(&time::format_description::well_known::Rfc3339).ok())
        .unwrap_or_default()
}

fn is_escrow(r: &SyncRecord) -> bool {
    r.entity_type == crate::ESCROW_ETYPE && r.uuid == crate::ESCROW_UUID
}

/// `escrow/{hex(salt)}` + raw body for this device's escrow record.
fn escrow_object(r: &SyncRecord) -> Result<(String, Vec<u8>), String> {
    let raw = hex::decode(r.blob.as_deref().unwrap_or_default()).map_err(|e| format!("[S3] BAD_ESCROW: {e}"))?;
    let salt = raw.get(1..1 + crate::SALT_LEN).ok_or("[S3] BAD_ESCROW: too short")?;
    Ok((format!("escrow/{}", hex::encode(salt)), raw))
}

/// Run `f` over `items` with at most CONCURRENCY in flight; results in input order.
async fn bounded<T, R, F, Fut>(items: Vec<T>, f: F) -> Vec<Result<R, String>>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> Fut,
    Fut: Future<Output = Result<R, String>> + Send + 'static,
{
    let n = items.len();
    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set = JoinSet::new();
    for (i, item) in items.into_iter().enumerate() {
        let sem = Arc::clone(&sem);
        let fut = f(item);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await;
            (i, fut.await)
        });
    }
    let mut out: Vec<Option<Result<R, String>>> = (0..n).map(|_| None).collect();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((i, r)) => out[i] = Some(r),
            Err(e) => eprintln!("[S3] request task failed: {e}"),
        }
    }
    out.into_iter()
        .map(|r| r.unwrap_or_else(|| Err("[S3] REQUEST_ABORTED".to_string())))
        .collect()
}

// ---------------------------------------------------------------------------
// The transport
// ---------------------------------------------------------------------------

pub struct ObjectSync<S: ObjectStore> {
    store: Arc<S>,
    /// Bucket prefix without leading/trailing `/`; may be empty.
    root: String,
}

impl<S: ObjectStore> ObjectSync<S> {
    pub fn new(store: Arc<S>, root: &str) -> Self {
        Self { store, root: root.trim_matches('/').to_string() }
    }

    fn base(&self) -> String {
        if self.root.is_empty() {
            String::new()
        } else {
            format!("{}/", self.root)
        }
    }

    fn partition(&self, profile: &str) -> Result<String, String> {
        if !valid_partition(profile) {
            return Err(format!("[S3] BAD_PROFILE_ID: {profile:?} can't be used as a bucket path"));
        }
        Ok(format!("{}{profile}/", self.base()))
    }

    async fn load_index(&self, part: &str) -> Result<Index, String> {
        let objects = self.store.list(part).await?;
        if objects.len() > MAX_KEYS {
            return Err(format!("[S3] BAD_RESPONSE: {} objects under {part} — refusing to sync", objects.len()));
        }
        Ok(build_index(&objects, part))
    }

    async fn put_all(&self, items: Vec<(String, Vec<u8>)>) -> Vec<Result<(), String>> {
        let store = Arc::clone(&self.store);
        bounded(items, move |(key, body)| {
            let store = Arc::clone(&store);
            async move { store.put(&key, body).await }
        })
        .await
    }

    async fn get_all(&self, keys: Vec<String>) -> Vec<Result<Option<Vec<u8>>, String>> {
        let store = Arc::clone(&self.store);
        bounded(keys, move |key| {
            let store = Arc::clone(&store);
            async move { store.get(&key).await }
        })
        .await
    }

    async fn delete_all(&self, keys: Vec<String>) -> Vec<Result<(), String>> {
        let store = Arc::clone(&self.store);
        bounded(keys, move |key| {
            let store = Arc::clone(&store);
            async move { store.delete(&key).await }
        })
        .await
    }
}

#[async_trait]
impl<S: ObjectStore> SyncTransport for ObjectSync<S> {
    async fn exchange(&self, profile: &str, push: &[SyncRecord], name: Option<&str>) -> Result<ExchangeOut, String> {
        let part = self.partition(profile)?;
        let idx = self.load_index(&part).await?;
        let records: Vec<SyncRecord> = push.iter().filter(|r| !is_escrow(r)).cloned().collect();
        let plan = plan_exchange(&records, &idx.versions, now_secs());

        // Upload groups, each finished before the next starts. Bookkeeping goes
        // first: data without an escrow can't be restored on a new device.
        let mut groups: Vec<Vec<(String, Vec<u8>)>> = Vec::new();
        let mut first = Vec::new();
        if let Some(e) = push.iter().find(|r| is_escrow(r)) {
            let (rel, body) = escrow_object(e)?;
            if !idx.escrows.contains(&rel) {
                first.push((format!("{part}{rel}"), body));
            }
        }
        if let (Some(n), false) = (name, idx.meta) {
            let body = serde_json::to_vec(&serde_json::json!({ "name": n })).map_err(|e| format!("[S3] META: {e}"))?;
            first.push((format!("{part}meta.json"), body));
        }
        groups.push(first);
        let mut current: Option<usize> = None;
        for (rec, rel) in &plan.puts {
            let body = if rec.deleted {
                Vec::new()
            } else {
                hex::decode(rec.blob.as_deref().unwrap_or_default())
                    .map_err(|e| format!("[S3] BAD_RECORD {}: {e}", rec.uuid))?
            };
            let g = upload_group(rec);
            if current != Some(g) {
                groups.push(Vec::new());
                current = Some(g);
            }
            groups.last_mut().expect("just pushed").push((format!("{part}{rel}"), body));
        }
        for group in groups.into_iter().filter(|g| !g.is_empty()) {
            let results = self.put_all(group).await;
            let failed: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
            if let Some(first_err) = failed.first() {
                return Err(format!(
                    "[S3] PARTIAL_PUSH: {} upload(s) failed; nothing was merged and the next sync retries — {first_err}",
                    failed.len()
                ));
            }
        }

        let keys: Vec<String> = plan.fetch.iter().map(|v| format!("{part}{}", v.rel)).collect();
        let bodies = self.get_all(keys).await;
        let mut remote = plan.tombs;
        for (v, body) in plan.fetch.into_iter().zip(bodies) {
            match body? {
                // Superseded and cleaned up between our LIST and GET; the
                // newer version arrives with the next sync.
                None => continue,
                Some(b) if b.len() > MAX_BODY_BYTES => {
                    eprintln!("[S3] skipped oversized object {} ({} bytes)", v.rel, b.len());
                }
                Some(b) => remote.push(SyncRecord {
                    uuid: v.uuid,
                    entity_type: v.etype,
                    updated_at: v.stamp,
                    deleted: false,
                    blob: Some(hex::encode(b)),
                }),
            }
        }

        // Best-effort cleanup; anything left is retried next sync.
        if !plan.gc.is_empty() {
            let keys: Vec<String> = plan.gc.iter().map(|rel| format!("{part}{rel}")).collect();
            for r in self.delete_all(keys).await {
                if let Err(e) = r {
                    eprintln!("[S3] cleanup skipped: {e}");
                }
            }
        }
        Ok(ExchangeOut { remote, pushed: plan.puts.len() })
    }

    async fn index(&self, profile: &str) -> Result<Vec<SyncRecord>, String> {
        let part = self.partition(profile)?;
        let idx = self.load_index(&part).await?;
        Ok(newest(&idx.versions)
            .into_values()
            .map(|v| SyncRecord {
                uuid: v.uuid.clone(),
                entity_type: v.etype.clone(),
                updated_at: v.stamp.clone(),
                deleted: v.deleted,
                blob: None,
            })
            .collect())
    }

    async fn escrows(&self, profile: &str) -> Result<Vec<String>, String> {
        let part = self.partition(profile)?;
        let idx = self.load_index(&part).await?;
        let keys: Vec<String> = idx.escrows.iter().map(|rel| format!("{part}{rel}")).collect();
        let mut out = Vec::new();
        for body in self.get_all(keys).await {
            if let Some(b) = body? {
                out.push(hex::encode(b));
            }
        }
        Ok(out)
    }

    async fn list_profiles(&self) -> Result<Vec<SyncProfileInfo>, String> {
        let base = self.base();
        let mut out = Vec::new();
        for dir in self.store.list_dirs(&base).await? {
            let Some(id) = dir.strip_prefix(&base).and_then(|d| d.strip_suffix('/')) else { continue };
            if !valid_partition(id) {
                continue;
            }
            let idx = self.load_index(&dir).await?;
            if idx.versions.is_empty() && idx.escrows.is_empty() && !idx.meta {
                continue;
            }
            let winners = newest(&idx.versions);
            let live = winners.values().filter(|v| !v.deleted).count();
            let mut name = id.to_string();
            if idx.meta {
                if let Some(body) = self.store.get(&format!("{dir}meta.json")).await? {
                    if let Some(n) = serde_json::from_slice::<serde_json::Value>(&body)
                        .ok()
                        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(str::to_string))
                    {
                        if crate::validate_profile_name(&n).is_ok() {
                            name = n;
                        }
                    }
                }
            }
            out.push(SyncProfileInfo {
                profile: id.to_string(),
                name,
                records: winners.len() as i64,
                live_records: live as i64,
                last_updated: rfc3339(idx.newest_upload),
            });
        }
        Ok(out)
    }

    async fn delete_profile(&self, profile: &str) -> Result<i64, String> {
        let part = self.partition(profile)?;
        let objects = self.store.list(&part).await?;
        let total = objects.len();
        let results = self.delete_all(objects.into_iter().map(|o| o.key).collect()).await;
        let failed: Vec<&String> = results.iter().filter_map(|r| r.as_ref().err()).collect();
        if let Some(first_err) = failed.first() {
            return Err(format!("[S3] DELETE_FAILED: {} of {total} object(s) could not be deleted — {first_err}", failed.len()));
        }
        Ok(total as i64)
    }
}

// ---------------------------------------------------------------------------
// In-memory store for tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod mem {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// A bucket in a BTreeMap (S3 lists in key order too), with fault injection.
    #[derive(Default)]
    pub(crate) struct MemStore {
        pub objects: Mutex<BTreeMap<String, (Vec<u8>, i64)>>,
        /// Fail every PUT once this many have succeeded.
        pub fail_puts_after: Mutex<Option<usize>>,
        pub puts: AtomicUsize,
    }

    impl MemStore {
        pub fn new() -> Arc<Self> {
            Arc::new(Self::default())
        }
        pub fn keys(&self) -> Vec<String> {
            self.objects.lock().unwrap().keys().cloned().collect()
        }
        /// Pretend every object was uploaded `secs` earlier.
        pub fn age_all(&self, secs: i64) {
            for v in self.objects.lock().unwrap().values_mut() {
                v.1 -= secs;
            }
        }
    }

    #[async_trait]
    impl ObjectStore for MemStore {
        async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, String> {
            Ok(self
                .objects
                .lock()
                .unwrap()
                .iter()
                .filter(|(k, _)| k.starts_with(prefix))
                .map(|(k, (_, lm))| ObjectInfo { key: k.clone(), last_modified: *lm })
                .collect())
        }
        async fn list_dirs(&self, prefix: &str) -> Result<Vec<String>, String> {
            let mut dirs: Vec<String> = self
                .objects
                .lock()
                .unwrap()
                .keys()
                .filter_map(|k| k.strip_prefix(prefix))
                .filter_map(|rest| rest.split_once('/').map(|(d, _)| format!("{prefix}{d}/")))
                .collect();
            dirs.dedup();
            Ok(dirs)
        }
        async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
            Ok(self.objects.lock().unwrap().get(key).map(|(b, _)| b.clone()))
        }
        async fn put(&self, key: &str, body: Vec<u8>) -> Result<(), String> {
            if let Some(n) = *self.fail_puts_after.lock().unwrap() {
                if self.puts.load(Ordering::SeqCst) >= n {
                    return Err("[S3] NETWORK: injected failure".into());
                }
            }
            self.puts.fetch_add(1, Ordering::SeqCst);
            self.objects.lock().unwrap().insert(key.to_string(), (body, now_secs()));
            Ok(())
        }
        async fn delete(&self, key: &str) -> Result<(), String> {
            self.objects.lock().unwrap().remove(key);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mem::MemStore;
    use super::*;

    fn rec(uuid: &str, etype: &str, stamp: &str, deleted: bool) -> SyncRecord {
        SyncRecord {
            uuid: uuid.into(),
            entity_type: etype.into(),
            updated_at: stamp.into(),
            deleted,
            blob: (!deleted).then(|| hex::encode(format!("blob-{uuid}-{stamp}"))),
        }
    }

    fn ver(uuid: &str, stamp: &str, deleted: bool, last_modified: i64) -> Version {
        Version {
            etype: "servers".into(),
            uuid: uuid.into(),
            stamp: stamp.into(),
            deleted,
            rel: version_rel("servers", uuid, stamp, deleted),
            last_modified,
        }
    }

    const S1: &str = "000000000000001:00000:a";
    const S2: &str = "000000000000002:00000:b";

    #[test]
    fn keys_round_trip_every_real_stamp_shape() {
        for stamp in ["001790902406000:00003:9f2c4a1b0d3e5f67", "000000000000001:00000:backfill", "000000000000000:00000:0"] {
            let rel = version_rel("monitor_configs", "ab-12", stamp, true);
            let Some(Entry::Version(v)) = parse_rel(&rel, 7) else { panic!("{rel} must parse") };
            assert_eq!((v.etype.as_str(), v.uuid.as_str(), v.stamp.as_str(), v.deleted, v.last_modified), ("monitor_configs", "ab-12", stamp, true, 7));
        }
    }

    #[test]
    fn keys_we_did_not_write_are_ignored() {
        let slash_stamp = hex::encode("a/b");
        for rel in [
            "r/servers/u1/zz.rec".to_string(),                 // not hex
            format!("r/servers/u1/{}.rec", hex::encode(S1).to_uppercase()),
            format!("r/SERVERS/u1/{}.rec", hex::encode(S1)),
            format!("r/servers/../{}.rec", hex::encode(S1)),
            format!("r/servers/u1/{}.txt", hex::encode(S1)),
            format!("r/servers/u1/x/{}.rec", hex::encode(S1)),
            format!("r/servers/u1/{slash_stamp}.rec"),
            format!("r/servers/{}/{}.rec", "u".repeat(65), hex::encode(S1)),
            "escrow/../../x".to_string(),
            "escrow/ABCD".to_string(),
            "meta.json.bak".to_string(),
        ] {
            assert!(parse_rel(&rel, 0).is_none(), "{rel} must be ignored");
        }
        assert!(!valid_partition("../x") && !valid_partition("") && !valid_partition(".probe"));
    }

    #[test]
    fn plan_uploads_only_what_the_bucket_lacks_and_fetches_only_newer() {
        let local = vec![rec("mine", "servers", S1, false), rec("shared", "servers", S1, false), rec("stale", "servers", S1, false)];
        let remote = vec![ver("shared", S1, false, 1), ver("stale", S2, false, 1), ver("theirs", S1, false, 1)];
        let plan = plan_exchange(&local, &remote, 10);
        let put: Vec<&str> = plan.puts.iter().map(|(r, _)| r.uuid.as_str()).collect();
        let mut fetch: Vec<&str> = plan.fetch.iter().map(|v| v.uuid.as_str()).collect();
        fetch.sort();
        // 'stale' is uploaded too: it's this device's losing version, kept as superseded.
        assert_eq!(put.len(), 2);
        assert!(put.contains(&"mine") && put.contains(&"stale"));
        assert_eq!(fetch, ["stale", "theirs"]);
        assert!(plan.tombs.is_empty() && plan.gc.is_empty());
    }

    #[test]
    fn a_tombstone_beats_a_record_at_the_same_stamp_and_needs_no_download() {
        let local = vec![rec("u", "servers", S1, false)];
        let remote = vec![ver("u", S1, false, 1), ver("u", S1, true, 1)];
        let plan = plan_exchange(&local, &remote, 10);
        assert!(plan.fetch.is_empty());
        assert_eq!(plan.tombs.len(), 1);
        assert!(plan.tombs[0].deleted && plan.tombs[0].blob.is_none());
    }

    #[test]
    fn local_state_is_the_newest_of_row_and_tombstone() {
        // Re-created after an older delete: the live row is current; nothing to fetch.
        let local = vec![rec("u", "servers", S2, false), rec("u", "servers", S1, true)];
        let remote = vec![ver("u", S2, false, 1), ver("u", S1, true, 1)];
        let plan = plan_exchange(&local, &remote, 10);
        assert!(plan.puts.is_empty() && plan.fetch.is_empty() && plan.tombs.is_empty());
    }

    #[test]
    fn cleanup_removes_only_superseded_versions_past_retention() {
        let old = 1_000;
        let now = old + RETENTION_SECS + 1;
        let remote = vec![
            ver("a", S1, false, old),       // superseded + old → delete
            ver("a", S2, false, old),       // winner → keep, however old
            ver("b", S1, false, now - 10),  // superseded but recent → keep
            ver("b", S2, true, old),        // winning tombstone → keep
            ver("c", S1, false, 0),         // superseded? no — sole version
        ];
        let plan = plan_exchange(&[], &remote, now);
        assert_eq!(plan.gc, vec![version_rel("servers", "a", S1, false)]);
    }

    #[test]
    fn uploads_go_referents_first_and_tombstones_last() {
        let local = vec![rec("t", "servers", S1, true), rec("s", "servers", S1, false), rec("c", "credentials", S1, false), rec("k", "ssh_keys", S1, false)];
        let plan = plan_exchange(&local, &[], 10);
        let order: Vec<&str> = plan.puts.iter().map(|(r, _)| r.uuid.as_str()).collect();
        assert_eq!(order, ["k", "c", "s", "t"]);
    }

    #[tokio::test]
    async fn a_failed_upload_stops_before_later_groups_and_reports_partial_push() {
        let store = MemStore::new();
        *store.fail_puts_after.lock().unwrap() = Some(1);
        let sync = ObjectSync::new(Arc::clone(&store), "root");
        let push = vec![rec("k", "ssh_keys", S1, false), rec("c", "credentials", S1, false), rec("s", "servers", S1, false)];
        let err = sync.exchange("p1", &push, None).await.err().expect("must fail");
        assert!(err.starts_with("[S3] PARTIAL_PUSH"), "{err}");
        let keys = store.keys();
        assert_eq!(keys.len(), 1, "only the first group landed: {keys:?}");
        assert!(keys[0].contains("/r/ssh_keys/"));

        // Retry once the store recovers: nothing is re-sent that already landed.
        *store.fail_puts_after.lock().unwrap() = None;
        let out = sync.exchange("p1", &push, None).await.unwrap();
        assert_eq!(out.pushed, 2);
    }

    #[tokio::test]
    async fn escrow_is_kept_per_salt_and_meta_is_written_once() {
        let store = MemStore::new();
        let sync = ObjectSync::new(Arc::clone(&store), "/root/");
        let escrow = |salt: u8| SyncRecord {
            uuid: crate::ESCROW_UUID.into(),
            entity_type: crate::ESCROW_ETYPE.into(),
            updated_at: "000000000000000:00000:0".into(),
            deleted: false,
            blob: Some(hex::encode([vec![2u8], vec![salt; crate::SALT_LEN], vec![9u8; 40]].concat())),
        };
        sync.exchange("p1", &[escrow(1)], Some("work")).await.unwrap();
        sync.exchange("p1", &[escrow(1)], Some("renamed")).await.unwrap();
        // A restored device has its own vault salt: both escrows coexist.
        sync.exchange("p1", &[escrow(2)], Some("work")).await.unwrap();
        let keys = store.keys();
        assert_eq!(keys.iter().filter(|k| k.starts_with("root/p1/escrow/")).count(), 2, "{keys:?}");
        assert_eq!(sync.escrows("p1").await.unwrap().len(), 2);
        let meta = store.objects.lock().unwrap().get("root/p1/meta.json").unwrap().0.clone();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&meta).unwrap()["name"], "work");
        // Bookkeeping isn't counted as pushed records.
        assert_eq!(sync.exchange("p1", &[escrow(1)], None).await.unwrap().pushed, 0);
    }

    #[tokio::test]
    async fn listing_and_deleting_profiles() {
        let store = MemStore::new();
        let sync = ObjectSync::new(Arc::clone(&store), "root");
        sync.exchange("p1", &[rec("a", "servers", S1, false), rec("b", "servers", S1, true)], Some("work")).await.unwrap();
        sync.exchange("p2", &[rec("c", "notes", S1, false)], Some("bad name!")).await.unwrap();
        store.objects.lock().unwrap().insert("root/.probe-x".into(), (vec![], 1));
        store.objects.lock().unwrap().insert("root/../evil/meta.json".into(), (vec![], 1));

        let mut list = sync.list_profiles().await.unwrap();
        list.sort_by(|a, b| a.profile.cmp(&b.profile));
        let summary: Vec<(&str, &str, i64, i64)> =
            list.iter().map(|p| (p.profile.as_str(), p.name.as_str(), p.records, p.live_records)).collect();
        assert_eq!(summary, [("p1", "work", 2, 1), ("p2", "p2", 1, 1)]);
        assert!(!list[0].last_updated.is_empty());

        assert_eq!(sync.delete_profile("p1").await.unwrap(), 3);
        assert!(store.keys().iter().all(|k| !k.starts_with("root/p1/")));
        assert!(sync.delete_profile("../p2").await.is_err());
    }
}
