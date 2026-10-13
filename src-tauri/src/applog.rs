//! The app's log file: `submarine.log` in the OS log folder (macOS
//! `~/Library/Logs/<identifier>/`, Linux `~/.local/share/<identifier>/logs/`,
//! Windows `%LOCALAPPDATA%\<identifier>\logs\`). One line per event,
//! `<UTC time> <LEVEL> [AREA] message`, so a failed sync or restore can be read
//! back after the fact instead of from a screenshot. Rotated at 1 MB with one
//! older file kept; created readable by the owner only.
//!
//! Callers must never pass secrets: passwords, passphrases, keys, tokens,
//! credentials-file contents, record blobs. As a second line of defence every
//! line goes through `redact`, which blanks anything shaped like one. Logging
//! never fails the caller — a line that can't be written is dropped (it still
//! goes to stderr for `tauri dev`).

use std::borrow::Cow;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use regex::{Captures, Regex};

const FILE_NAME: &str = "submarine.log";
const MAX_BYTES: u64 = 1024 * 1024;
/// Longest message kept; anything past it is cut.
const MAX_MESSAGE: usize = 4000;

static DIR: OnceLock<PathBuf> = OnceLock::new();
/// One writer at a time, so lines never interleave and rotation can't race.
static WRITE: Mutex<()> = Mutex::new(());

/// Start writing to `dir` (created if missing). Until this runs, lines only go
/// to stderr — which is all unit tests ever see.
pub fn init(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let _ = DIR.set(dir);
}

pub fn path() -> Option<PathBuf> {
    DIR.get().map(|d| d.join(FILE_NAME))
}

pub fn error(area: &str, message: impl AsRef<str>) {
    write("ERROR", area, message.as_ref());
}

pub fn warn(area: &str, message: impl AsRef<str>) {
    write("WARN", area, message.as_ref());
}

pub fn info(area: &str, message: impl AsRef<str>) {
    write("INFO", area, message.as_ref());
}

/// Log at the level a UI log type names: `error`, `warn`/`warning`, else info.
pub fn by_kind(kind: &str, area: &str, message: impl AsRef<str>) {
    let level = match kind {
        "error" => "ERROR",
        "warn" | "warning" => "WARN",
        _ => "INFO",
    };
    write(level, area, message.as_ref());
}

fn write(level: &str, area: &str, message: &str) {
    let line = format_line(&now(), level, area, message);
    eprintln!("{line}");
    if let Some(dir) = DIR.get() {
        let _guard = WRITE.lock().unwrap_or_else(|p| p.into_inner());
        append(dir, &line, MAX_BYTES);
    }
}

fn now() -> String {
    let t = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    )
}

/// One line per event: line breaks inside the message become ` | `, then the
/// message is redacted and capped.
fn format_line(time: &str, level: &str, area: &str, message: &str) -> String {
    let flat = message.replace("\r\n", " | ").replace(['\n', '\r'], " | ");
    let mut text = redact(&flat).into_owned();
    if text.len() > MAX_MESSAGE {
        let mut cut = MAX_MESSAGE;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    format!("{time} {level:<5} [{area}] {text}")
}

fn append(dir: &Path, line: &str, max_bytes: u64) {
    let file = dir.join(FILE_NAME);
    if std::fs::metadata(&file).is_ok_and(|m| m.len() >= max_bytes) {
        let old = dir.join(format!("{FILE_NAME}.1"));
        // Windows' rename won't replace an existing file.
        let _ = std::fs::remove_file(&old);
        let _ = std::fs::rename(&file, &old);
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    if let Ok(mut f) = options.open(&file) {
        let _ = writeln!(f, "{line}");
    }
}

/// Blank anything in `text` shaped like a secret. A backstop, not a licence:
/// it knows the shapes this app handles (PEM / PuTTY private keys, SigV4
/// headers and query parameters, bearer tokens, `name=value` secrets, S3 error
/// echoes, 32-byte hex keys, long hex blobs, base64 runs), and keeps what's
/// needed to debug — record and partition ids, HLC stamps, error codes, paths.
pub(crate) fn redact(text: &str) -> Cow<'_, str> {
    static RULES: OnceLock<Vec<(Regex, &'static str)>> = OnceLock::new();
    static BASE64: OnceLock<Regex> = OnceLock::new();
    let rules = RULES.get_or_init(|| {
        let rule = |re: &str, with: &'static str| (Regex::new(re).expect("redaction pattern"), with);
        vec![
            rule(
                r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----.*?(?:-----END [A-Z0-9 ]*PRIVATE KEY-----|$)",
                "[redacted private key]",
            ),
            rule(r"(?i)\bPrivate-(?:Lines|MAC):.*", "[redacted private key]"),
            rule(r"(?i)\b(authorization)\s*[:=]\s*[^|]*", "$1: [redacted] "),
            rule(r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=-]+", "$1 [redacted]"),
            rule(
                r#"(?i)\b(password|passphrase|passwd|secret|secret_key|secret_access_key|access_key|access_key_id|aws_access_key_id|aws_secret_access_key|token|access_token|refresh_token|claim_token|api_key|apikey|private_key|signature|x-amz-signature|x-amz-credential|x-amz-security-token)\b("?\s*[:=]\s*"?)[^\s,;&"'<|]+"#,
                "$1$2[redacted]",
            ),
            rule(
                r"(?i)<(AWSAccessKeyId|SignatureProvided|StringToSign|StringToSignBytes|CanonicalRequest|CanonicalRequestBytes|SecretAccessKey|SessionToken)>[^<]*",
                "<$1>[redacted]",
            ),
            rule(r"\b(?:[0-9A-Fa-f]{64}|[0-9A-Fa-f]{100,})\b", "[redacted]"),
        ]
    });
    let mut out = Cow::Borrowed(text);
    for (re, with) in rules {
        if re.is_match(&out) {
            out = Cow::Owned(re.replace_all(&out, *with).into_owned());
        }
    }
    // Base64 runs that mix upper case, lower case and digits — key material,
    // tokens, S3 secrets. Lower-case hex ids and stamps never mix cases.
    let base64 = BASE64.get_or_init(|| Regex::new(r"[A-Za-z0-9+/=]{40,}").expect("redaction pattern"));
    let mixed = |s: &str| {
        s.bytes().any(|b| b.is_ascii_uppercase())
            && s.bytes().any(|b| b.is_ascii_lowercase())
            && s.bytes().any(|b| b.is_ascii_digit())
    };
    if base64.find_iter(&out).any(|m| mixed(m.as_str())) {
        out = Cow::Owned(
            base64
                .replace_all(&out, |c: &Captures| if mixed(&c[0]) { "[redacted]".to_string() } else { c[0].to_string() })
                .into_owned(),
        );
    }
    out
}

/// The frontend's way in: Activity Log entries and errors shown on screens
/// that have no Activity Log (the profile picker, cloud panel). Same
/// redaction as every other line.
#[tauri::command]
pub fn app_log(level: String, area: String, message: String) {
    let area: String = area
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
        .take(16)
        .collect::<String>()
        .to_uppercase();
    let area = if area.is_empty() { "UI".to_string() } else { area };
    by_kind(&level, &area, &message);
}

/// Where the log file is, for Settings → Activity Log.
#[tauri::command]
pub fn app_log_path() -> Option<String> {
    path().map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_redacted() {
        let cases = [
            "key: -----BEGIN OPENSSH PRIVATE KEY----- | b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAABAAAAMwAAAAtzc2gtZW | -----END OPENSSH PRIVATE KEY----- done",
            "-----BEGIN RSA PRIVATE KEY-----MIIEowIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gun",
            "PuTTY-User-Key-File-3: ssh-ed25519 | Private-Lines: 1 | AAAAIFq3vzN4mJpFXX4WfFjcI1Vh1nU9dc2K | Private-MAC: 8c1e",
            "Authorization: AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, SignedHeaders=host;range;x-amz-date, Signature=fe5f80f77d5fa3beca038a248ff027d0445342fe2855ddc963176630326f1024",
            "GET /b/k?X-Amz-Credential=AKIAIOSFODNN7EXAMPLE%2F20130524&X-Amz-Signature=aeeed9bbccd4d02ee5c0109b86d86835f995330da4c265957d157751f604d404",
            "header Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl",
            "password=hunter2-AKIAIOSFODNN7EXAMPLE",
            r#"{"secret_key": "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY", "token":"tok_AKIAIOSFODNN7EXAMPLE"}"#,
            "secret_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            "<Error><Code>SignatureDoesNotMatch</Code><AWSAccessKeyId>AKIAIOSFODNN7EXAMPLE</AWSAccessKeyId><StringToSign>AWS4-HMAC-SHA256 20130524T000000Z fe5f80f77d5fa3beca038a248ff027d0</StringToSign></Error>",
            "a stray secret wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY in a message",
            "dek 9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 leaked",
            "blob 02aa0f3c9b9d7e1f00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899aabbccdd",
        ];
        let secrets = [
            "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ",
            "MIIEowIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gun",
            "AAAAIFq3vzN4mJpFXX4WfFjcI1Vh1nU9dc2K",
            "AKIAIOSFODNN7EXAMPLE",
            "fe5f80f77d5fa3beca038a248ff027d0",
            "aeeed9bbccd4d02ee5c0109b86d86835",
            "eyJhbGciOiJIUzI1NiJ9",
            "hunter2",
            "wJalrXUtnFEMI",
            "tok_",
            "9f86d081884c7d659a2feaa0c55ad015",
            "02aa0f3c9b9d7e1f0011",
        ];
        for case in cases {
            let out = redact(case);
            for secret in secrets {
                assert!(!out.contains(secret), "{secret:?} survived in {out:?}");
            }
            assert!(out.contains("[redacted"), "nothing redacted in {out:?}");
        }
    }

    #[test]
    fn what_debugging_needs_is_kept() {
        for keep in [
            "[S3] SIGNATURE_MISMATCH: the storage server rejected the request signature — check the secret key and the region (using 'default')",
            "[S3] ACCESS_DENIED: Access Denied — check the access key, secret key and region (using 'us-east-1'), and that the key may use bucket 'b'",
            "[SYNC] ESCROW_UNSEAL_FAILED: wrong password for this profile",
            "Can't use 'E2e' here: this device's disk ignores letter case, so it would be the same file as your profile 'e2e'. Pick another name.",
            "[S3] skipped oversized object r/servers/7111f9976d77518339992bdf603141ca/3030313739313234313036393839323a30303030303a62646561363665666666353363303630.rec (9000000 bytes)",
            "'9ce4e5a5854b1d646fd7972a46d6d8c3' synced: pushed 3, pulled 1 (412 ms)",
            "[S3] CREDENTIALS_FILE: can't read /Users/someone/.config/submarine-sync/missing: No such file or directory (os error 2)",
            "Connected to bucket 'b' at https://us-ord-1.linodeobjects.com (region default) — write, read, list and delete all work.",
            "Attempting Password Authentication...",
            "SSH key format: OpenSSH",
        ] {
            assert_eq!(redact(keep), keep);
        }
    }

    #[test]
    fn lines_are_flat_capped_and_rotated() {
        let line = format_line("T", "WARN", "S3", "first\nsecond\r\nthird");
        assert_eq!(line, "T WARN  [S3] first | second | third");
        let long = format_line("T", "INFO", "X", &"é".repeat(MAX_MESSAGE));
        assert!(long.ends_with('…') && long.len() < MAX_MESSAGE + 40);

        let dir = std::env::temp_dir().join(format!("submarine-log-{}", crate::new_entity_uuid()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..20 {
            append(&dir, &format!("line {i:02} {}", "x".repeat(40)), 200);
        }
        let current = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        let old = std::fs::read_to_string(dir.join(format!("{FILE_NAME}.1"))).unwrap();
        assert!(current.contains("line 19") && (current.len() as u64) < 200 + 60);
        assert!(!old.is_empty() && !old.contains("line 19"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.join(FILE_NAME)).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
