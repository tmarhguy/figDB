//! Structured tracing bootstrap.
//!
//! All binaries call [`init_tracing`] once at startup. Format is
//! JSON in production (`FIG_LOG_JSON=1`) and pretty-human otherwise.
//! `RUST_LOG` / `FIG_LOG` control filtering.

use tracing_subscriber::{fmt, EnvFilter};

/// Initialize global tracing subscriber. Safe to call once; subsequent calls are no-ops.
pub fn init_tracing() {
    let filter = EnvFilter::try_from_env("FIG_LOG")
        .or_else(|_| EnvFilter::try_from_env("RUST_LOG"))
        .unwrap_or_else(|_| EnvFilter::new("info,fig=debug"));

    let json = std::env::var("FIG_LOG_JSON")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    // Ignore double-init errors (e.g. in tests).
    if json {
        let _ = fmt()
            .json()
            .with_env_filter(filter)
            .with_target(true)
            .with_file(true)
            .with_line_number(true)
            .try_init();
    } else {
        let _ = fmt().with_env_filter(filter).with_target(true).try_init();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        init_tracing();
        init_tracing();
    }
}
