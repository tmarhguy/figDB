//! `fig-storage` — FigDB milestone skeleton (see docs/architecture/overview.md).
//!
//! Commit 01 establishes the crate boundary; real implementation lands in its
//! designated milestone commit (Commits 02–20). This placeholder keeps the
//! workspace compiling, tested, and documented from day one.

/// Crate name for logging/metrics labels.
pub const CRATE_NAME: &str = "fig-storage";

/// Placeholder health check used by workspace smoke tests.
pub fn health() -> &'static str {
    CRATE_NAME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_health() {
        assert_eq!(health(), CRATE_NAME);
    }
}
