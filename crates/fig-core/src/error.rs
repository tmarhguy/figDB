//! Shared error conventions for FigDB.
//!
//! - `Error` is the single error type re-exported by `fig-core`.
//! - Infrastructure failures (I/O) are transparent.
//! - User-visible failures (invalid args, not found, timeouts) are explicit variants.
//! - Every RPC/server boundary maps `Error` to a structured status with a stable code.

use thiserror::Error;

/// Result alias used across all FigDB crates.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error for FigDB.
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid argument supplied by client (key too large, empty, bad range, ...).
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// Key or resource not found.
    #[error("not found: {0}")]
    NotFound(String),

    /// Operation timed out / deadline exceeded.
    #[error("timeout: {0}")]
    Timeout(String),

    /// Node is not the leader for this shard; caller should redirect.
    #[error("not leader: expected {expected}, got {got}")]
    NotLeader { expected: String, got: String },

    /// Stale routing / epoch; client must refresh shard map.
    #[error("stale route: {0}")]
    StaleRoute(String),

    /// Transaction conflict / abort.
    #[error("conflict: {0}")]
    Conflict(String),

    /// Storage corruption detected (checksum, torn write, bad SSTable).
    #[error("corruption: {0}")]
    Corruption(String),

    /// I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Serialization failure.
    #[error("codec: {0}")]
    Codec(String),

    /// Internal invariant violation (bug).
    #[error("internal: {0}")]
    Internal(String),
}

impl Error {
    /// Stable machine-readable code for RPC/metrics mapping.
    pub fn code(&self) -> &'static str {
        match self {
            Error::InvalidArgument(_) => "INVALID_ARGUMENT",
            Error::NotFound(_) => "NOT_FOUND",
            Error::Timeout(_) => "TIMEOUT",
            Error::NotLeader { .. } => "NOT_LEADER",
            Error::StaleRoute(_) => "STALE_ROUTE",
            Error::Conflict(_) => "CONFLICT",
            Error::Corruption(_) => "CORRUPTION",
            Error::Io(_) => "IO",
            Error::Codec(_) => "CODEC",
            Error::Internal(_) => "INTERNAL",
        }
    }

    /// Whether the operation is safe to retry with the same request ID.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Error::Timeout(_) | Error::NotLeader { .. } | Error::StaleRoute(_)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_stable() {
        assert_eq!(Error::NotFound("k".into()).code(), "NOT_FOUND");
        assert!(Error::Timeout("t".into()).is_retryable());
        assert!(!Error::Corruption("c".into()).is_retryable());
    }
}
