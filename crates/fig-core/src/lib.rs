//! `fig-core`: shared types, errors, config, and observability bootstrap.
//!
//! All other crates depend on this crate for consistent conventions.

pub mod config;
pub mod error;
pub mod tracing_util;

pub use config::Config;
pub use error::{Error, Result};
