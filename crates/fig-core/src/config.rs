//! Node/cluster configuration.
//!
//! Single source of truth for limits documented in `internal.md` §3:
//! key size, value size, transaction size, batch size, request size, timeout.

use serde::{Deserialize, Serialize};

/// Global limits for keys, values, batches, and requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Limits {
    /// Max key size in bytes (default 1 MiB... spec says arbitrary bytes; we bound for safety).
    pub max_key_bytes: usize,
    /// Max value size in bytes.
    pub max_value_bytes: usize,
    /// Max transaction write-set bytes.
    pub max_txn_bytes: usize,
    /// Max batch ops per request.
    pub max_batch_ops: usize,
    /// Max request payload bytes.
    pub max_request_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_key_bytes: 64 * 1024,
            max_value_bytes: 4 * 1024 * 1024,
            max_txn_bytes: 16 * 1024 * 1024,
            max_batch_ops: 1000,
            max_request_bytes: 32 * 1024 * 1024,
        }
    }
}

/// Top-level node configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Human-readable node ID, e.g. `node-1`.
    pub node_id: String,
    /// Address to listen on, e.g. `127.0.0.1:7001`.
    pub listen_addr: String,
    /// Data directory for WAL/SSTables/manifest.
    pub data_dir: String,
    /// Request timeout in milliseconds.
    pub request_timeout_ms: u64,
    /// Limits.
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            node_id: "node-1".to_string(),
            listen_addr: "127.0.0.1:7001".to_string(),
            data_dir: "./data/node-1".to_string(),
            request_timeout_ms: 5000,
            limits: Limits::default(),
        }
    }
}

impl Config {
    /// Validate bounds; returns [`crate::Error::InvalidArgument`] on violation.
    pub fn validate(&self) -> crate::Result<()> {
        if self.node_id.is_empty() {
            return Err(crate::Error::InvalidArgument("node_id empty".into()));
        }
        if self.listen_addr.is_empty() {
            return Err(crate::Error::InvalidArgument("listen_addr empty".into()));
        }
        Ok(())
    }

    /// Validate a user key against limits.
    pub fn check_key(&self, key: &[u8]) -> crate::Result<()> {
        if key.is_empty() {
            return Err(crate::Error::InvalidArgument("key empty".into()));
        }
        if key.len() > self.limits.max_key_bytes {
            return Err(crate::Error::InvalidArgument(format!(
                "key too large: {} > {}",
                key.len(),
                self.limits.max_key_bytes
            )));
        }
        Ok(())
    }

    /// Validate a user value against limits.
    pub fn check_value(&self, value: &[u8]) -> crate::Result<()> {
        if value.len() > self.limits.max_value_bytes {
            return Err(crate::Error::InvalidArgument(format!(
                "value too large: {} > {}",
                value.len(),
                self.limits.max_value_bytes
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_validates() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn rejects_empty_and_oversize_keys() {
        let cfg = Config::default();
        assert!(cfg.check_key(&[]).is_err());
        assert!(cfg
            .check_key(&vec![0u8; cfg.limits.max_key_bytes + 1])
            .is_err());
        assert!(cfg.check_key(b"ok").is_ok());
    }
}
