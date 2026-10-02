//! `ObjectStore` over any S3-compatible service (Linode Object Storage, AWS,
//! MinIO, R2, B2): plain REST calls signed with SigV4 headers (`sigv4.rs`),
//! sent over the shared reqwest/rustls stack.
//!
//! Credentials come from an s3cmd-format config file (`[default]` with
//! `access_key` / `secret_key`, plus `host_base`, `use_https`,
//! `bucket_location` as endpoint/region defaults). They are read when a sync
//! starts, held in memory for that sync only, and never written anywhere by the
//! app — the same file the `s3cmd`-based tooling already uses.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use zeroize::Zeroizing;

use crate::object_sync::{ObjectInfo, ObjectStore};
use crate::sigv4;
use crate::sync_backend::S3Settings;

const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;
/// A 403 with the server's clock this far from ours is reported as clock skew.
const SKEW_SECS: i64 = 300;

/// Attempts per request: transport failures and 5xx (Linode answers bursts
/// with `503 SlowDown`) are retried with exponential backoff.
const ATTEMPTS: u32 = 5;

/// One client per store, i.e. per sync: its pooled connections live on the
/// runtime that made them, so a process-wide client can outlive them. Redirects
/// are off — an S3 redirect means "wrong endpoint/region", and a followed
/// request would carry a signature for the wrong host anyway.
fn new_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("submarine-app/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| format!("[S3] NETWORK: {e}"))
}

/// 250 ms, 500 ms, 1 s, 2 s … plus up to 100 ms of jitter so parallel
/// requests that were throttled together don't all come back together.
fn backoff(attempt: u32) -> Duration {
    Duration::from_millis((250u64 << (attempt - 1).min(4)) + rand::random::<u64>() % 100)
}

// ---------------------------------------------------------------------------
// s3cmd config
// ---------------------------------------------------------------------------

/// What we use from an s3cmd config file. Deliberately no Debug: it holds the secret.
pub(crate) struct S3cmdConfig {
    pub access_key: String,
    pub secret_key: Zeroizing<String>,
    pub host_base: Option<String>,
    pub use_https: bool,
    pub bucket_location: Option<String>,
}

/// `~/` → the user's home directory.
pub(crate) fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(dirs) = directories::BaseDirs::new() {
            return dirs.home_dir().join(rest);
        }
    }
    PathBuf::from(path)
}

/// Parse the `[default]` section of an s3cmd config.
pub(crate) fn parse_s3cmd_config(text: &str) -> Result<S3cmdConfig, String> {
    let mut in_default = false;
    let mut get = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.starts_with('[') {
            in_default = line == "[default]";
            continue;
        }
        if !in_default {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            get.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let non_empty = |k: &str| get.get(k).filter(|v| !v.is_empty()).cloned();
    let (Some(access_key), Some(secret_key)) = (non_empty("access_key"), non_empty("secret_key")) else {
        return Err("no access_key / secret_key in its [default] section".into());
    };
    Ok(S3cmdConfig {
        access_key,
        secret_key: Zeroizing::new(secret_key),
        host_base: non_empty("host_base"),
        // s3cmd's own default is HTTPS.
        use_https: non_empty("use_https").map_or(true, |v| !v.eq_ignore_ascii_case("false")),
        bucket_location: non_empty("bucket_location"),
    })
}

pub(crate) fn read_s3cmd_config(path: &str) -> Result<S3cmdConfig, String> {
    let full = expand_home(path);
    let text = Zeroizing::new(
        std::fs::read_to_string(&full)
            .map_err(|e| format!("[S3] CREDENTIALS_FILE: can't read {}: {e}", full.display()))?,
    );
    parse_s3cmd_config(&text).map_err(|e| format!("[S3] CREDENTIALS_FILE: {}: {e}", full.display()))
}

/// s3cmd's mapping from `bucket_location` to a SigV4 region. Only a starting
/// guess: a server that wants another region says so, and the store retries
/// with it (Ceph-based services such as Linode's want `default`).
fn region_from_location(location: Option<&str>) -> String {
    match location {
        None | Some("") | Some("US") => "us-east-1".into(),
        Some("EU") => "eu-west-1".into(),
        Some(other) => other.to_string(),
    }
}

/// Endpoint URL + region, settings overrides first, then the s3cmd file.
pub(crate) fn endpoint_and_region(settings: &S3Settings, cfg: &S3cmdConfig) -> Result<(String, String), String> {
    let endpoint = match settings.endpoint.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => e.to_string(),
        None => {
            let host = cfg.host_base.as_deref().ok_or(
                "[S3] NOT_CONFIGURED: no endpoint — set one, or add host_base to the credentials file",
            )?;
            format!("{}://{host}", if cfg.use_https { "https" } else { "http" })
        }
    };
    let region = match settings.region.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(r) => r.to_string(),
        None => region_from_location(cfg.bucket_location.as_deref()),
    };
    Ok((endpoint, region))
}

// ---------------------------------------------------------------------------
// XML (S3 responses are flat, machine-written documents)
// ---------------------------------------------------------------------------

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16).ok())
                .unwrap_or_else(|| entity.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Text of every `<tag>…</tag>` in `xml` (not nested in itself).
fn xml_all<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find(&open) {
        let body = &rest[s + open.len()..];
        let Some(e) = body.find(&close) else { break };
        out.push(&body[..e]);
        rest = &body[e + close.len()..];
    }
    out
}

fn xml_first(xml: &str, tag: &str) -> Option<String> {
    xml_all(xml, tag).first().map(|s| xml_unescape(s.trim()))
}

/// The `<Code>` of an S3 XML error body, if any.
fn xml_code(body: &str) -> Option<String> {
    xml_first(body, "Code")
}

struct ListPage {
    /// (key, LastModified)
    contents: Vec<(String, String)>,
    prefixes: Vec<String>,
    next_token: Option<String>,
}

/// A ListObjectsV2 response requested with `encoding-type=url`, so keys and
/// prefixes are percent-encoded.
fn parse_list(xml: &str) -> ListPage {
    let decode = |s: String| percent_encoding::percent_decode_str(&s).decode_utf8_lossy().into_owned();
    let contents = xml_all(xml, "Contents")
        .into_iter()
        .filter_map(|c| Some((decode(xml_first(c, "Key")?), xml_first(c, "LastModified").unwrap_or_default())))
        .collect();
    let prefixes = xml_all(xml, "CommonPrefixes").into_iter().filter_map(|c| xml_first(c, "Prefix").map(decode)).collect();
    let truncated = xml_first(xml, "IsTruncated").is_some_and(|t| t == "true");
    let next_token = xml_first(xml, "NextContinuationToken").filter(|t| truncated && !t.is_empty());
    ListPage { contents, prefixes, next_token }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Seconds between the server's `Date` header and our clock.
fn clock_offset(date_header: Option<&str>) -> Option<i64> {
    let server = time::OffsetDateTime::parse(date_header?, &time::format_description::well_known::Rfc2822).ok()?;
    Some(server.unix_timestamp() - time::OffsetDateTime::now_utc().unix_timestamp())
}

/// Map an S3 error response to a stable `[S3] CODE: message`.
pub(crate) fn describe_error(status: u16, code: Option<&str>, skew: Option<i64>, bucket: &str, region: &str) -> String {
    let skewed = skew.is_some_and(|s| s.abs() > SKEW_SECS);
    match (status, code) {
        (_, Some("RequestTimeTooSkewed")) | (403, _) if skewed || code == Some("RequestTimeTooSkewed") => format!(
            "[S3] CLOCK_SKEW: this computer's clock is {}s off the storage server's — fix the system time and sync again",
            skew.unwrap_or(0).abs()
        ),
        (_, Some("NoSuchBucket")) => format!("[S3] BUCKET_NOT_FOUND: bucket '{bucket}' doesn't exist at this endpoint"),
        (_, Some("SignatureDoesNotMatch" | "AuthorizationHeaderMalformed")) => format!(
            "[S3] SIGNATURE_MISMATCH: the storage server rejected the request signature — check the secret key and the region (using '{region}')"
        ),
        (_, Some("InvalidAccessKeyId")) => "[S3] ACCESS_DENIED: the access key isn't recognised by this endpoint".into(),
        // Some gateways (Linode's among them) report a bad signature this way too.
        (403, c) => format!(
            "[S3] ACCESS_DENIED: {} — check the access key, secret key and region (using '{region}'), and that the key may use bucket '{bucket}'",
            c.unwrap_or("forbidden")
        ),
        (301 | 307 | 308, _) | (_, Some("PermanentRedirect")) => {
            "[S3] BAD_RESPONSE: the server redirected the request — the endpoint or region doesn't match this bucket".into()
        }
        (404, _) => format!("[S3] BUCKET_NOT_FOUND: bucket '{bucket}' or its path wasn't found"),
        (s, c) => format!("[S3] BAD_RESPONSE: HTTP {s}{}", c.map(|c| format!(" {c}")).unwrap_or_default()),
    }
}

/// Transport failures with their cause chain ("error sending request" alone
/// says nothing). The URL is stripped; with header auth it carries no secret,
/// but keys/paths have no business in the activity log either.
fn network_error(e: reqwest::Error) -> String {
    let unreachable = e.is_connect() || e.is_timeout();
    let e = e.without_url();
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    if unreachable {
        format!("[S3] ENDPOINT_UNREACHABLE: {msg}")
    } else {
        format!("[S3] NETWORK: {msg}")
    }
}

fn parse_last_modified(s: &str) -> i64 {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map(|t| t.unix_timestamp())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

pub struct S3Store {
    http: reqwest::Client,
    scheme: String,
    /// `host[:port]` of the endpoint.
    authority: String,
    bucket: String,
    path_style: bool,
    access_key: String,
    secret_key: Zeroizing<String>,
    /// Starts from the settings/s3cmd guess; replaced once if the server names
    /// the region it wants (the same discovery s3cmd and the AWS SDKs do).
    region: Mutex<String>,
}

/// A response with its body read (bodies here are small: records, listings).
struct Reply {
    status: u16,
    body: Vec<u8>,
}

impl S3Store {
    pub fn new(settings: &S3Settings) -> Result<Self, String> {
        if settings.bucket.trim().is_empty() {
            return Err("[S3] NOT_CONFIGURED: no bucket set".into());
        }
        Self::with_config(settings, read_s3cmd_config(&settings.credentials_file)?)
    }

    fn with_config(settings: &S3Settings, cfg: S3cmdConfig) -> Result<Self, String> {
        let (endpoint, region) = endpoint_and_region(settings, &cfg)?;
        let url = url::Url::parse(&endpoint).map_err(|e| format!("[S3] NOT_CONFIGURED: bad endpoint {endpoint:?}: {e}"))?;
        let host = url.host_str().ok_or_else(|| format!("[S3] NOT_CONFIGURED: bad endpoint {endpoint:?}"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(format!("[S3] NOT_CONFIGURED: endpoint {endpoint:?} must be http(s)"));
        }
        Ok(Self {
            http: new_client()?,
            scheme: url.scheme().to_string(),
            authority: url.port().map_or_else(|| host.to_string(), |p| format!("{host}:{p}")),
            bucket: settings.bucket.trim().to_string(),
            path_style: settings.path_style,
            access_key: cfg.access_key,
            secret_key: cfg.secret_key,
            region: Mutex::new(region),
        })
    }

    fn region(&self) -> String {
        self.region.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// (endpoint, region in use) — for the connection test's report.
    pub fn describe(&self) -> (String, String) {
        (format!("{}://{}", self.scheme, self.authority), self.region())
    }

    /// URL for `key` ("" = the bucket itself) with `query`, path pre-encoded so
    /// the signed path is exactly the sent path.
    fn url_for(&self, key: &str, query: &[(&str, &str)]) -> Result<url::Url, String> {
        let path = sigv4::uri_encode(key, true);
        let mut s = if self.path_style {
            format!("{}://{}/{}/{path}", self.scheme, self.authority, sigv4::uri_encode(&self.bucket, false))
        } else {
            format!("{}://{}.{}/{path}", self.scheme, self.bucket, self.authority)
        };
        if !query.is_empty() {
            s.push('?');
            let pairs: Vec<String> = query
                .iter()
                .map(|(k, v)| format!("{}={}", sigv4::uri_encode(k, false), sigv4::uri_encode(v, false)))
                .collect();
            s.push_str(&pairs.join("&"));
        }
        url::Url::parse(&s).map_err(|e| format!("[S3] NOT_CONFIGURED: can't address bucket {:?}: {e}", self.bucket))
    }

    /// Send one signed request. Success and 404s come back as a `Reply`
    /// (callers decide what a missing object means); every other status is
    /// mapped to an `[S3]` error — after one retry if the server named a
    /// different region than the one we signed for. Every request here is
    /// idempotent (immutable PUTs, GET, DELETE, LIST), so dropped connections
    /// and 5xx answers are retried with backoff, re-signed each time.
    async fn call(&self, method: reqwest::Method, key: &str, query: &[(&str, &str)], body: Option<Vec<u8>>) -> Result<Reply, String> {
        let url = self.url_for(key, query)?;
        let payload = body.as_deref().map_or_else(|| sigv4::EMPTY_SHA256.to_string(), sigv4::sha256_hex);
        let mut retried = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let region = self.region();
            let signed = sigv4::sign(
                method.as_str(),
                &url,
                &region,
                &self.access_key,
                &self.secret_key,
                &payload,
                time::OffsetDateTime::now_utc(),
            );
            let mut req = self
                .http
                .request(method.clone(), url.clone())
                .header("x-amz-date", signed.amz_date)
                .header("x-amz-content-sha256", signed.content_sha256)
                .header(reqwest::header::AUTHORIZATION, signed.authorization);
            if let Some(b) = &body {
                req = req.body(b.clone());
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) if attempt < ATTEMPTS => {
                    eprintln!("[S3] retrying after: {}", network_error(e));
                    tokio::time::sleep(backoff(attempt)).await;
                    continue;
                }
                Err(e) => return Err(network_error(e)),
            };
            let status = resp.status().as_u16();
            if matches!(status, 500 | 502 | 503 | 504) && attempt < ATTEMPTS {
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
            let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
            let (date, bucket_region) = (header("date"), header("x-amz-bucket-region"));
            if resp.content_length().is_some_and(|n| n as usize > MAX_BODY_BYTES) {
                return Err(format!("[S3] BAD_RESPONSE: response for {key:?} is larger than {MAX_BODY_BYTES} bytes"));
            }
            let bytes = resp.bytes().await.map_err(network_error)?.to_vec();
            if (200..300).contains(&status) {
                return Ok(Reply { status, body: bytes });
            }
            let text = String::from_utf8_lossy(&bytes);
            let code = xml_code(&text);
            if status == 404 && code.as_deref() != Some("NoSuchBucket") {
                return Ok(Reply { status, body: Vec::new() });
            }
            let wanted = bucket_region.or_else(|| xml_first(&text, "Region")).filter(|r| !r.is_empty() && *r != region);
            if let (false, Some(r)) = (retried, wanted) {
                if let Ok(mut g) = self.region.lock() {
                    *g = r;
                }
                retried = true;
                continue;
            }
            return Err(describe_error(status, code.as_deref(), clock_offset(date.as_deref()), &self.bucket, &region));
        }
    }

    async fn list_pages(&self, prefix: &str, delimiter: bool) -> Result<Vec<ListPage>, String> {
        let mut pages = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2"), ("encoding-type", "url"), ("prefix", prefix)];
            if delimiter {
                query.push(("delimiter", "/"));
            }
            if let Some(t) = token.as_deref() {
                query.push(("continuation-token", t));
            }
            let reply = self.call(reqwest::Method::GET, "", &query, None).await?;
            if reply.status == 404 {
                return Err(describe_error(404, None, None, &self.bucket, &self.region()));
            }
            let page = parse_list(&String::from_utf8_lossy(&reply.body));
            token = page.next_token.clone();
            pages.push(page);
            if token.is_none() {
                return Ok(pages);
            }
        }
    }
}

#[async_trait]
impl ObjectStore for S3Store {
    async fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>, String> {
        Ok(self
            .list_pages(prefix, false)
            .await?
            .into_iter()
            .flat_map(|p| p.contents)
            .map(|(key, lm)| ObjectInfo { key, last_modified: parse_last_modified(&lm) })
            .collect())
    }

    async fn list_dirs(&self, prefix: &str) -> Result<Vec<String>, String> {
        Ok(self.list_pages(prefix, true).await?.into_iter().flat_map(|p| p.prefixes).collect())
    }

    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let reply = self.call(reqwest::Method::GET, key, &[], None).await?;
        Ok((reply.status != 404).then_some(reply.body))
    }

    async fn put(&self, key: &str, body: Vec<u8>) -> Result<(), String> {
        self.call(reqwest::Method::PUT, key, &[], Some(body)).await.map(|_| ())
    }

    async fn delete(&self, key: &str) -> Result<(), String> {
        self.call(reqwest::Method::DELETE, key, &[], None).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_default_section_of_an_s3cmd_config() {
        let cfg = parse_s3cmd_config(
            "[other]\naccess_key = nope\n\n[default]\n# comment\naccess_key = AK\nsecret_key=SK\nhost_base = us-ord-10.linodeobjects.com\nuse_https = True\nbucket_location = US\n",
        )
        .unwrap();
        assert_eq!((cfg.access_key.as_str(), cfg.secret_key.as_str()), ("AK", "SK"));
        let (endpoint, region) = endpoint_and_region(&S3Settings::default(), &cfg).unwrap();
        assert_eq!(endpoint, "https://us-ord-10.linodeobjects.com");
        assert_eq!(region, "us-east-1");

        let overridden = S3Settings { endpoint: Some("http://127.0.0.1:9000".into()), region: Some("us-ord-10".into()), ..S3Settings::default() };
        assert_eq!(
            endpoint_and_region(&overridden, &cfg).unwrap(),
            ("http://127.0.0.1:9000".to_string(), "us-ord-10".to_string())
        );
        assert!(parse_s3cmd_config("[default]\naccess_key = AK\n").is_err(), "a missing secret is an error");
    }

    #[test]
    fn errors_map_to_stable_codes() {
        let d = |status, code, skew| describe_error(status, code, skew, "b", "us-east-1");
        assert!(d(403, Some("SignatureDoesNotMatch"), Some(2)).starts_with("[S3] SIGNATURE_MISMATCH"));
        assert!(d(403, Some("AccessDenied"), Some(3600)).starts_with("[S3] CLOCK_SKEW"));
        assert!(d(403, Some("RequestTimeTooSkewed"), None).starts_with("[S3] CLOCK_SKEW"));
        assert!(d(404, Some("NoSuchBucket"), None).starts_with("[S3] BUCKET_NOT_FOUND"));
        assert!(d(403, Some("InvalidAccessKeyId"), None).starts_with("[S3] ACCESS_DENIED"));
        assert!(d(301, None, None).starts_with("[S3] BAD_RESPONSE"));
        assert!(d(500, Some("InternalError"), None).starts_with("[S3] BAD_RESPONSE: HTTP 500 InternalError"));
        assert_eq!(xml_code("<Error><Code>NoSuchBucket</Code></Error>").as_deref(), Some("NoSuchBucket"));
        assert!(clock_offset(Some("Fri, 02 Oct 2026 00:54:33 GMT")).is_some(), "HTTP dates must parse");
    }

    #[test]
    fn parses_list_objects_v2() {
        let page = parse_list(
            "<?xml version=\"1.0\"?><ListBucketResult><Name>b</Name><Prefix>root%2F</Prefix><IsTruncated>true</IsTruncated>\
             <Contents><Key>root/p1/r/a%20b/x.rec</Key><LastModified>2026-10-02T00:54:33.000Z</LastModified><Size>3</Size></Contents>\
             <Contents><Key>root/p1/meta.json</Key><LastModified>2026-10-02T00:54:34.000Z</LastModified></Contents>\
             <CommonPrefixes><Prefix>root/p1/</Prefix></CommonPrefixes>\
             <NextContinuationToken>1x+/=&amp;y</NextContinuationToken></ListBucketResult>",
        );
        assert_eq!(page.contents[0].0, "root/p1/r/a b/x.rec");
        assert_eq!(parse_last_modified(&page.contents[0].1), 1_790_902_473);
        assert_eq!(page.contents.len(), 2);
        assert_eq!(page.prefixes, ["root/p1/"]);
        assert_eq!(page.next_token.as_deref(), Some("1x+/=&y"));
        let last = parse_list("<ListBucketResult><IsTruncated>false</IsTruncated><NextContinuationToken>z</NextContinuationToken></ListBucketResult>");
        assert!(last.next_token.is_none(), "a non-truncated page ends the listing");
        assert_eq!(xml_unescape("a&lt;b&#62;&#x41;&bogus"), "a<b>A&bogus");
    }

    #[test]
    fn addresses_buckets_both_ways() {
        let cfg = || parse_s3cmd_config("[default]\naccess_key=a\nsecret_key=s\nhost_base=h.example:9000\nuse_https=False\n").unwrap();
        let path = S3Store::with_config(&S3Settings { bucket: "bk".into(), ..Default::default() }, cfg()).unwrap();
        assert_eq!(path.url_for("p/r/x y.rec", &[("prefix", "a/b")]).unwrap().as_str(), "http://h.example:9000/bk/p/r/x%20y.rec?prefix=a%2Fb");
        let vhost = S3Store::with_config(&S3Settings { bucket: "bk".into(), path_style: false, ..Default::default() }, cfg()).unwrap();
        assert_eq!(vhost.url_for("", &[]).unwrap().as_str(), "http://bk.h.example:9000/");
    }

    // Against a real endpoint; skipped unless SUBMARINE_S3_IT=1 (see it_settings).
    #[tokio::test]
    async fn it_store_operations_pagination_and_errors() {
        let Some(settings) = crate::sync_engine_tests::it_settings() else { return };
        let store = std::sync::Arc::new(S3Store::new(&settings).unwrap());
        let base = format!("{}/", settings.prefix);

        // More than one listing page (S3 returns at most 1000 keys per page).
        let mut set = tokio::task::JoinSet::new();
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(16));
        for i in 0..1005 {
            let (store, sem) = (std::sync::Arc::clone(&store), std::sync::Arc::clone(&sem));
            let key = format!("{base}p{}/r/notes/u{i:04}/aa.rec", i % 2);
            set.spawn(async move {
                let _p = sem.acquire_owned().await;
                store.put(&key, vec![1, 2, 3]).await
            });
        }
        while let Some(r) = set.join_next().await {
            r.unwrap().unwrap();
        }
        let all = store.list(&base).await.unwrap();
        assert_eq!(all.len(), 1005, "every page must be read");
        assert!(all.iter().all(|o| o.last_modified > 0), "LastModified must parse");
        let mut dirs = store.list_dirs(&base).await.unwrap();
        dirs.sort();
        assert_eq!(dirs, [format!("{base}p0/"), format!("{base}p1/")]);

        let key = &all[0].key;
        assert_eq!(store.get(key).await.unwrap(), Some(vec![1, 2, 3]));
        assert_eq!(store.get(&format!("{base}missing")).await.unwrap(), None);
        store.delete(&format!("{base}missing")).await.unwrap();

        let wrong_bucket = S3Settings { bucket: "submarine-it-no-such-bucket".into(), ..settings.clone() };
        let err = S3Store::new(&wrong_bucket).unwrap().list(&base).await.unwrap_err();
        assert!(err.starts_with("[S3] BUCKET_NOT_FOUND") || err.starts_with("[S3] ACCESS_DENIED"), "{err}");

        let mut wrong_secret = read_s3cmd_config(&settings.credentials_file).unwrap();
        wrong_secret.secret_key = Zeroizing::new("wrong-secret".into());
        let err = S3Store::with_config(&settings, wrong_secret).unwrap().list(&base).await.unwrap_err();
        assert!(err.starts_with("[S3] SIGNATURE_MISMATCH") || err.starts_with("[S3] ACCESS_DENIED"), "{err}");

        // Leave the prefix empty.
        for o in store.list(&base).await.unwrap() {
            let (store, sem) = (std::sync::Arc::clone(&store), std::sync::Arc::clone(&sem));
            set.spawn(async move {
                let _p = sem.acquire_owned().await;
                store.delete(&o.key).await
            });
        }
        while let Some(r) = set.join_next().await {
            r.unwrap().unwrap();
        }
        assert!(store.list(&base).await.unwrap().is_empty());
    }
}
