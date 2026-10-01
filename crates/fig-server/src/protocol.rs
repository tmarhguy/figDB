//! TCP wire protocol: newline-delimited JSON, one request → one response.
//!
//! Keys and values are arbitrary bytes, so they travel base64-encoded
//! (standard alphabet). Every response carries the request `seq` for
//! pipelining, plus `ok` and — on failure — the stable [`fig_core::Error`]
//! code so clients can branch on `NOT_LEADER`/`TIMEOUT`/etc. without parsing
//! human text.
//!
//! ```text
//! {"seq":1,"op":"put","key_b64":"ay4=","value_b64":"dg=="}
//! {"seq":1,"ok":true}
//! {"seq":2,"op":"get","key_b64":"ay4="}
//! {"seq":2,"ok":true,"value_b64":"dg=="}
//! ```

use serde::{Deserialize, Serialize};

/// Max decoded request line: mirrors `Limits::max_request_bytes`.
pub const MAX_LINE_BYTES: usize = 32 * 1024 * 1024;
/// Default/max pairs returned by one `scan`.
pub const DEFAULT_SCAN_LIMIT: usize = 1000;
pub const MAX_SCAN_LIMIT: usize = 10_000;

/// One client request. `op` is one of
/// `put|delete|get|scan|sync|flush|compact|stats` (anything else is
/// `INVALID_ARGUMENT`, and the connection stays up).
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    pub seq: u64,
    pub op: String,
    #[serde(default)]
    pub key_b64: Option<String>,
    #[serde(default)]
    pub value_b64: Option<String>,
    #[serde(default)]
    pub start_b64: Option<String>,
    #[serde(default)]
    pub end_b64: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One server response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub seq: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value_b64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pairs: Option<Vec<Pair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<Stats>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merged: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pair {
    pub key_b64: String,
    pub value_b64: String,
}

/// `stats` payload: LSM counters plus live table count.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub flushes: u64,
    pub flushed_records: u64,
    pub flushed_bytes: u64,
    pub compactions: u64,
    pub compacted_records: u64,
    pub tables: usize,
}

impl Response {
    pub fn ok(seq: u64) -> Self {
        Self {
            seq,
            ok: true,
            code: None,
            message: None,
            value_b64: None,
            pairs: None,
            stats: None,
            removed: None,
            merged: None,
        }
    }

    pub fn err(seq: u64, code: &str, message: String) -> Self {
        Self {
            seq,
            ok: false,
            code: Some(code.to_string()),
            message: Some(message),
            value_b64: None,
            pairs: None,
            stats: None,
            removed: None,
            merged: None,
        }
    }
}

/// Decode standard-alphabet base64, mapping failures to an invalid-argument
/// message (never a panic on client bytes).
pub fn decode_b64(field: &str, s: &str) -> Result<Vec<u8>, String> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| format!("{field}: bad base64 ({e})"))
}

pub fn encode_b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
