//! `fig-core`: shared types, errors, config, and observability bootstrap.
//!
//! All other crates depend on this crate for consistent conventions.

pub mod config;
pub mod error;
pub mod kv;
pub mod ops;
pub mod reference;
pub mod tracing_util;

pub use config::Config;
pub use error::{Error, Result};
pub use kv::{MemoryKv, Op, OpResult};
pub use reference::ReferenceKv;
