//! Minimal S3 object upload: AWS Signature Version 4 (HMAC-SHA256) PUTs, with optional
//! S3-compatible endpoints (MinIO and friends). Only static credentials are supported —
//! `access_key`/`secret_key` from config or the standard `AWS_*` environment variables.
//!
//! The signing logic is verified against reference vectors produced by botocore 1.43.113
//! (`S3SigV4Auth`); no test here talks to AWS.
use anyhow::{bail, Context, Result};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct S3Config {
    pub bucket: String,
    /// Key prefix for uploaded objects (`results` -> `results/<job>/<artifact>`).
    pub prefix: String,
    pub region: String,
    /// S3-compatible endpoint (for example `http://127.0.0.1:9000`); AWS is used when unset.
    pub endpoint: Option<String>,
    /// Path-style addressing (`endpoint/bucket/key`) instead of virtual-host style.
    pub path_style: bool,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub session_token: Option<String>,
    /// Also upload the zip archive of the job artifacts.
    pub upload_archive: bool,
}

pub struct S3Upload<'a> {
    pub bucket: &'a str,
    pub prefix: &'a str,
    pub region: &'a str,
    pub endpoint: Option<&'a str>,
    pub path_style: bool,
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub session_token: Option<&'a str>,
}

pub struct SignedPut {
    pub url: String,
    pub content_sha256: String,
    pub date: String,
    pub authorization: String,
    pub session_token: Option<String>,
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// S3 key encoding: path separators stay, unreserved characters stay, everything else is
/// percent-encoded with uppercase hex (spaces are `%20`, not `+`).
pub fn encode_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    for byte in key.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

impl S3Upload<'_> {
    pub fn url_for(&self, key: &str) -> String {
        let path = format!("/{}", encode_key(key));
        match self.endpoint {
            Some(endpoint) => {
                let endpoint = endpoint.trim_end_matches('/');
                if self.path_style {
                    format!("{endpoint}/{}{path}", self.bucket)
                } else {
                    format!("{endpoint}{path}")
                }
            }
            None if self.path_style => {
                format!(
                    "https://s3.{}.amazonaws.com/{}{path}",
                    self.region, self.bucket
                )
            }
            None => format!(
                "https://{}.s3.{}.amazonaws.com{path}",
                self.bucket, self.region
            ),
        }
    }
}

/// `scheme://host[:port]/path` -> (`host[:port]`, `/path`).
fn split_url(url: &str) -> Result<(&str, &str)> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .context("URL has no scheme")?;
    match rest.split_once('/') {
        Some((host, _)) => Ok((host, &rest[host.len()..])),
        None => Ok((rest, "/")),
    }
}

/// SigV4 headers for a `PUT` with a known payload hash. `timestamp` is `YYYYMMDDTHHMMSSZ`.
pub fn sign_put(base: &S3Upload, key: &str, payload_sha256: &str, timestamp: &str) -> SignedPut {
    let url = base.url_for(key);
    let (host, path) = split_url(&url).expect("constructed URL is well-formed");
    let date_short = &timestamp[..8];
    let mut canonical_headers =
        format!("host:{host}\nx-amz-content-sha256:{payload_sha256}\nx-amz-date:{timestamp}\n");
    let mut signed_headers = "host;x-amz-content-sha256;x-amz-date".to_owned();
    if let Some(token) = base.session_token {
        canonical_headers.push_str(&format!("x-amz-security-token:{token}\n"));
        signed_headers.push_str(";x-amz-security-token");
    }
    let canonical_request =
        format!("PUT\n{path}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}");
    let scope = format!("{date_short}/{}/s3/aws4_request", base.region);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
        sha256_hex(canonical_request.as_bytes())
    );
    let mut signing_key = hmac_sha256(
        format!("AWS4{}", base.secret_key).as_bytes(),
        date_short.as_bytes(),
    );
    signing_key = hmac_sha256(&signing_key, base.region.as_bytes());
    signing_key = hmac_sha256(&signing_key, b"s3");
    signing_key = hmac_sha256(&signing_key, b"aws4_request");
    let signature = hex::encode(hmac_sha256(&signing_key, string_to_sign.as_bytes()));
    SignedPut {
        url,
        content_sha256: payload_sha256.to_owned(),
        date: timestamp.to_owned(),
        authorization: format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
            base.access_key
        ),
        session_token: base.session_token.map(str::to_owned),
    }
}

fn hash_file(path: &std::path::Path) -> Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buffer = vec![0u8; 1 << 20];
    let mut hasher = Sha256::new();
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Stream one object to S3. Returns the object URL.
pub fn put_object(base: &S3Upload, key: &str, path: &std::path::Path) -> Result<String> {
    let payload_sha256 = hash_file(path)?;
    let signed = sign_put(base, key, &payload_sha256, &utc_timestamp());
    let length = std::fs::metadata(path)?.len();
    let file = std::fs::File::open(path)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()?;
    let mut request = client
        .put(&signed.url)
        .header("x-amz-content-sha256", &signed.content_sha256)
        .header("x-amz-date", &signed.date)
        .header("authorization", &signed.authorization);
    if let Some(token) = &signed.session_token {
        request = request.header("x-amz-security-token", token);
    }
    let response = request
        .body(reqwest::blocking::Body::sized(file, length))
        .send()
        .with_context(|| format!("S3 PUT {}", signed.url))?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        bail!("S3 PUT {} failed: {status} {}", signed.url, body.trim());
    }
    Ok(signed.url)
}

/// Current UTC time as `YYYYMMDDTHHMMSSZ` (no external date dependency).
fn utc_timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}{month:02}{day:02}T{:02}{:02}{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-from-civil inverse: unix days -> (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
    const ACCESS: &str = "AKIDEXAMPLE";
    // Reference vectors generated with botocore 1.43.113 (S3SigV4Auth, clock fixed at
    // 20150830T123600Z) on 2026-10-14 so signing can be checked without touching AWS.
    const TIMESTAMP: &str = "20150830T123600Z";

    fn base<'a>(
        endpoint: Option<&'a str>,
        path_style: bool,
        token: Option<&'a str>,
    ) -> S3Upload<'a> {
        S3Upload {
            bucket: "bucket",
            prefix: "",
            region: "us-east-1",
            endpoint,
            path_style,
            access_key: ACCESS,
            secret_key: SECRET,
            session_token: token,
        }
    }

    #[test]
    fn signs_virtual_host_put_like_botocore() {
        let signed = sign_put(
            &base(None, false, None),
            "jobs/1/result.txt",
            "069ae2d08bdc442d9f912021ac6a1a20a3bde69a09469c412b2bb0595947d27f",
            TIMESTAMP,
        );
        assert_eq!(
            signed.url,
            "https://bucket.s3.us-east-1.amazonaws.com/jobs/1/result.txt"
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, \
             Signature=2365e34d46a6e1715b60fd5271b8043363fdb3de88b810e336fd82b403ab873a"
        );
    }

    #[test]
    fn signs_path_style_put_with_session_token_like_botocore() {
        let signed = sign_put(
            &base(Some("http://127.0.0.1:9000"), true, Some("SESSIONTOKEN")),
            "key.txt",
            "239f59ed55e737c77147cf55ad0c1b030b6d7ee748a7426952f9b852d5a935e5",
            TIMESTAMP,
        );
        assert_eq!(signed.url, "http://127.0.0.1:9000/bucket/key.txt");
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date;x-amz-security-token, \
             Signature=1b212fc2d3bbef3e536071dd50beed1c2c18e6dac892b59c03ac10e4c6b9d186"
        );
    }

    #[test]
    fn encodes_keys_and_signs_aws_root_endpoint_like_botocore() {
        let signed = sign_put(
            &base(None, false, None),
            "a b+c.txt",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            TIMESTAMP,
        );
        assert_eq!(
            signed.url,
            "https://bucket.s3.us-east-1.amazonaws.com/a%20b%2Bc.txt"
        );
        assert_eq!(
            signed.authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/s3/aws4_request, \
             SignedHeaders=host;x-amz-content-sha256;x-amz-date, \
             Signature=506c06eb4e374660339f5338a02bb5decb8fd13494b1a29fdd2ce625a01db0e5"
        );
        assert_eq!(encode_key("é/ü"), "%C3%A9/%C3%BC");
    }

    #[test]
    fn formats_utc_timestamps() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
        let stamp = utc_timestamp();
        assert_eq!(stamp.len(), 16);
        assert!(
            stamp.ends_with('Z') && stamp.as_bytes()[8] == b'T',
            "{stamp}"
        );
    }
}
