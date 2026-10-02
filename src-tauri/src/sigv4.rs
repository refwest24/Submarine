//! AWS Signature Version 4, header form, for S3 requests.
//!
//! Header auth (`Authorization: AWS4-HMAC-SHA256 …`) rather than presigned
//! URLs: every S3-compatible service accepts it, while some gateways don't
//! accept every presigned request — Linode's newer Object Storage endpoints
//! took presigned GET/PUT but rejected presigned bucket listings. Spec:
//! <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-header-based-auth.html>

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// SHA-256 of an empty body, the payload hash for GET/DELETE/LIST.
pub const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding as SigV4 wants it: unreserved characters stay, every
/// other byte becomes %XX (upper-case); `/` is kept only when `keep_slash`.
pub fn uri_encode(s: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b'/' if keep_slash => out.push('/'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `YYYYMMDDTHHMMSSZ`
pub fn amz_date(t: time::OffsetDateTime) -> String {
    let t = t.to_offset(time::UtcOffset::UTC);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// The headers that authenticate one request.
pub struct Signed {
    pub amz_date: String,
    pub content_sha256: String,
    pub authorization: String,
}

/// Sign `method url` whose body hashes to `payload_sha256`. `url` must be the
/// exact URL that is sent (its path already URI-encoded); the signed headers
/// are `host`, `x-amz-content-sha256` and `x-amz-date`.
pub fn sign(
    method: &str,
    url: &url::Url,
    region: &str,
    access_key: &str,
    secret_key: &str,
    payload_sha256: &str,
    now: time::OffsetDateTime,
) -> Signed {
    let amz_date = amz_date(now);
    let date = &amz_date[..8];
    let host = match url.port() {
        Some(p) => format!("{}:{p}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_string(),
    };
    let mut query: Vec<(String, String)> =
        url.query_pairs().map(|(k, v)| (uri_encode(&k, false), uri_encode(&v, false))).collect();
    query.sort();
    let canonical_query = query.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let signed_headers = "host;x-amz-content-sha256;x-amz-date";
    let canonical_request = format!(
        "{method}\n{}\n{canonical_query}\nhost:{host}\nx-amz-content-sha256:{payload_sha256}\nx-amz-date:{amz_date}\n\n{signed_headers}\n{payload_sha256}",
        url.path()
    );
    let scope = format!("{date}/{region}/s3/aws4_request");
    let string_to_sign = format!("AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));
    let k_date = hmac(format!("AWS4{secret_key}").as_bytes(), date.as_bytes());
    let k_region = hmac(&k_date, region.as_bytes());
    let k_service = hmac(&k_region, b"s3");
    let k_signing = hmac(&k_service, b"aws4_request");
    let signature = hex::encode(hmac(&k_signing, string_to_sign.as_bytes()));
    Signed {
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={access_key}/{scope},SignedHeaders={signed_headers},Signature={signature}"
        ),
        amz_date,
        content_sha256: payload_sha256.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The worked examples from AWS's SigV4 header-auth documentation.
    const AK: &str = "AKIAIOSFODNN7EXAMPLE";
    const SK: &str = "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";

    fn may_24_2013() -> time::OffsetDateTime {
        time::OffsetDateTime::from_unix_timestamp(1_369_353_600).unwrap()
    }

    fn signature(url: &str) -> String {
        let s = sign("GET", &url::Url::parse(url).unwrap(), "us-east-1", AK, SK, EMPTY_SHA256, may_24_2013());
        assert_eq!(s.amz_date, "20130524T000000Z");
        s.authorization.rsplit("Signature=").next().unwrap().to_string()
    }

    #[test]
    fn matches_aws_example_get_bucket_lifecycle() {
        assert_eq!(
            signature("https://examplebucket.s3.amazonaws.com/?lifecycle"),
            "fea454ca298b7da1c68078a5d1bdbfbbe0d65c699e0f91ac7a200a0136783543"
        );
    }

    #[test]
    fn matches_aws_example_list_objects() {
        assert_eq!(
            signature("https://examplebucket.s3.amazonaws.com/?max-keys=2&prefix=J"),
            "34b48302e7b5fa45bde8084f4b7868a86f0a534bc59db6670ed5711ef69dc6f7"
        );
    }

    #[test]
    fn encodes_like_sigv4() {
        assert_eq!(uri_encode("a b/c~d+e=", true), "a%20b/c~d%2Be%3D");
        assert_eq!(uri_encode("x/y", false), "x%2Fy");
        assert_eq!(sha256_hex(b""), EMPTY_SHA256);
    }
}
