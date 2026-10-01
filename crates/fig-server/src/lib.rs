//! `fig-server` shared library: wire protocol + connection serving.

pub mod protocol;
pub mod server;

pub use protocol::{Request, Response, Stats};
pub use server::{handle_conn, resolve_dir, serve, Db};
