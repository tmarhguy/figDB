//! `fig-server`: listen on TCP, serve one manifest-backed LSM database.
//!
//! ```bash
//! cargo run -p fig-server --bin fig-server -- --dir /tmp/fig-srv --addr 127.0.0.1:7001
//! ```
//!
//! Durability note: the server runs `FsyncPolicy::Never` and syncs only on
//! the `sync` op — same rule as the library. A write is acknowledged iff a
//! later `sync` covered it; an unsynced tail may be lost to SIGKILL.

use fig_server::resolve_dir;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut dir_arg: Option<String> = None;
    let mut addr = "127.0.0.1:7001".to_string();
    let mut threshold: usize = 4 * 1024 * 1024;
    let mut compact_interval_ms: u64 = 1000;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir_arg = args.next(),
            "--addr" => {
                addr = args.next().unwrap_or_else(|| {
                    eprintln!("--addr needs a value");
                    std::process::exit(2);
                });
            }
            "--flush-threshold" => {
                threshold = args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                    eprintln!("--flush-threshold needs a byte count");
                    std::process::exit(2);
                });
            }
            "--compact-interval-ms" => {
                compact_interval_ms =
                    args.next().and_then(|s| s.parse().ok()).unwrap_or_else(|| {
                        eprintln!("--compact-interval-ms needs a millisecond count (0 disables)");
                        std::process::exit(2);
                    });
            }
            "--help" | "-h" => {
                println!(
                    "usage: fig-server [--dir DIR] [--addr HOST:PORT] [--flush-threshold BYTES] [--compact-interval-ms MS]"
                );
                return Ok(());
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    let dir = resolve_dir(dir_arg);
    std::fs::create_dir_all(&dir)?;
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    eprintln!("fig-server: {} serving {}", addr, dir.display());
    fig_server::serve(listener, &dir, threshold, compact_interval_ms).await
}
