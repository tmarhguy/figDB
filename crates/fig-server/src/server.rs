//! Single-node TCP server: one `Database` behind a read-write lock, one task
//! per connection, line-delimited JSON per `protocol`.
//!
//! Concurrency model (deliberate):
//! - Reads (`get`/`scan`/`stats`) take the lock shared: they run concurrently
//!   and never block each other. Table iteration uses LSM snapshots, so even
//!   a concurrent flush/compact is invisible to them.
//! - Writes (`put`/`delete`/`sync`/`flush`/manual `compact`) take it
//!   exclusively: single-writer serialization is correctness for the WAL.
//! - Every DB call runs on `spawn_blocking`: the engine does file I/O and
//!   fsyncs, which must never stall Tokio workers. No lock guard is ever held
//!   across an `.await`.
//! - Compaction runs on a background timer (not inline in `flush()`), so
//!   merges never stall the write path. See ADR-005.

use crate::protocol::{
    self, Pair, Request, Response, Stats, DEFAULT_SCAN_LIMIT, MAX_LINE_BYTES, MAX_SCAN_LIMIT,
};
use fig_core::{Config, Error};
use fig_storage::Database;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

/// Shared database handle. `std` (not Tokio) lock: guards never cross an
/// `.await` — each op runs to completion on a blocking thread.
pub type Db = Arc<RwLock<Database>>;

/// Open (or create) the database at `dir` with server defaults and serve
/// `listener` forever. `flush_threshold` bounds the memtable before a
/// foreground flush; fsync policy is `Never` — durability is explicit via
/// the `sync` op and periodic client syncs, exactly like the library API.
/// `compact_interval_ms` drives the background compaction timer (`0`
/// disables it; inline auto-compaction is off in server mode regardless).
pub async fn serve(
    listener: TcpListener,
    dir: &Path,
    flush_threshold: usize,
    compact_interval_ms: u64,
) -> anyhow::Result<()> {
    let mut db = open_db(dir, flush_threshold)?;
    // The background task owns compaction; the write path only ever appends.
    db.set_auto_compact(false);
    let db: Db = Arc::new(RwLock::new(db));
    if compact_interval_ms > 0 {
        let bg = Arc::clone(&db);
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(std::time::Duration::from_millis(compact_interval_ms));
            loop {
                tick.tick().await;
                let bg = Arc::clone(&bg);
                // Merge work runs on a blocking thread; failures are logged,
                // never fatal — the next tick retries.
                let _ = tokio::task::spawn_blocking(move || match bg.write() {
                    Ok(mut db) => {
                        if let Err(e) = db.background_compact() {
                            tracing::warn!(error = %e, "background compact failed");
                        }
                    }
                    Err(e) => tracing::warn!(error = %e, "background compact: lock poisoned"),
                })
                .await;
            }
        });
    }
    tracing::info!(
        addr = %listener.local_addr().map(|a| a.to_string()).unwrap_or_default(),
        dir = %dir.display(),
        "fig-server listening"
    );
    loop {
        let (stream, peer) = listener.accept().await?;
        tracing::info!(%peer, "client connected");
        let db = Arc::clone(&db);
        tokio::spawn(async move {
            if let Err(e) = handle_conn(stream, db).await {
                tracing::info!(%peer, error = %e, "client disconnected");
            }
        });
    }
}

fn open_db(dir: &Path, flush_threshold: usize) -> anyhow::Result<Database> {
    let cfg = Config {
        data_dir: dir.to_string_lossy().to_string(),
        ..Default::default()
    };
    Database::open(
        dir,
        cfg,
        fig_wal::FsyncPolicy::Never,
        flush_threshold.max(1),
    )
    .map_err(|e| anyhow::anyhow!("open database at {}: {e}", dir.display()))
}

/// Serve exactly one connection (used by tests with an ephemeral listener).
pub async fn handle_conn(stream: TcpStream, db: Db) -> anyhow::Result<()> {
    let (read_half, write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut writer = write_half;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            return Ok(()); // EOF: clean client close.
        }
        let owned = line.trim_end_matches(['\r', '\n']).to_string();
        let db = Arc::clone(&db);
        // Blocking engine work (WAL appends, fsyncs, merges) stays off the
        // async workers; the response comes back over the join handle.
        let resp = tokio::task::spawn_blocking(move || dispatch(&db, &owned)).await?;
        let mut out = serde_json::to_vec(&resp)?;
        out.push(b'\n');
        writer.write_all(&out).await?;
        writer.flush().await?;
    }
}

/// Execute one request line to a response. Synchronous: runs on a blocking
/// thread with a lock flavor matched to the op (shared for reads).
fn dispatch(db: &Db, line: &str) -> Response {
    if line.len() > MAX_LINE_BYTES {
        return Response::err(
            0,
            "INVALID_ARGUMENT",
            format!("line exceeds {MAX_LINE_BYTES} bytes"),
        );
    }
    let req: Request = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => return Response::err(0, "INVALID_ARGUMENT", format!("bad request: {e}")),
    };
    let seq = req.seq;
    let fail = |e: Error| Response::err(seq, e.code(), e.to_string());
    let need = |v: &Option<String>, field: &str| -> Result<Vec<u8>, String> {
        match v {
            Some(s) => protocol::decode_b64(field, s),
            None => Err(format!("{field} missing")),
        }
    };
    let poisoned = || Response::err(seq, "INTERNAL", "lock poisoned".to_string());

    match req.op.as_str() {
        "put" => {
            let k = match need(&req.key_b64, "key_b64") {
                Ok(k) => k,
                Err(m) => return Response::err(seq, "INVALID_ARGUMENT", m),
            };
            let v = match need(&req.value_b64, "value_b64") {
                Ok(v) => v,
                Err(m) => return Response::err(seq, "INVALID_ARGUMENT", m),
            };
            let mut db = match db.write() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.put(k, v) {
                Ok(()) => Response::ok(seq),
                Err(e) => fail(e),
            }
        }
        "delete" => {
            let k = match need(&req.key_b64, "key_b64") {
                Ok(k) => k,
                Err(m) => return Response::err(seq, "INVALID_ARGUMENT", m),
            };
            let mut db = match db.write() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.delete(&k) {
                Ok(removed) => {
                    let mut r = Response::ok(seq);
                    r.removed = Some(removed);
                    r
                }
                Err(e) => fail(e),
            }
        }
        "get" => {
            let k = match need(&req.key_b64, "key_b64") {
                Ok(k) => k,
                Err(m) => return Response::err(seq, "INVALID_ARGUMENT", m),
            };
            let db = match db.read() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.get(&k) {
                Ok(Some(v)) => {
                    let mut r = Response::ok(seq);
                    r.value_b64 = Some(protocol::encode_b64(&v));
                    r
                }
                Ok(None) => Response::err(seq, "NOT_FOUND", "key not found".to_string()),
                Err(e) => fail(e),
            }
        }
        "scan" => {
            let start = match need(&req.start_b64, "start_b64") {
                Ok(s) => s,
                Err(m) => return Response::err(seq, "INVALID_ARGUMENT", m),
            };
            let end = match need(&req.end_b64, "end_b64") {
                Ok(s) => s,
                Err(m) => return Response::err(seq, "INVALID_ARGUMENT", m),
            };
            let limit = req
                .limit
                .unwrap_or(DEFAULT_SCAN_LIMIT)
                .clamp(1, MAX_SCAN_LIMIT);
            let db = match db.read() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.scan(&start, &end) {
                Ok(pairs) => {
                    let mut r = Response::ok(seq);
                    r.pairs = Some(
                        pairs
                            .into_iter()
                            .take(limit)
                            .map(|(k, v)| Pair {
                                key_b64: protocol::encode_b64(&k),
                                value_b64: protocol::encode_b64(&v),
                            })
                            .collect(),
                    );
                    r
                }
                Err(e) => fail(e),
            }
        }
        "sync" => {
            let mut db = match db.write() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.sync() {
                Ok(()) => Response::ok(seq),
                Err(e) => fail(e),
            }
        }
        "flush" => {
            let mut db = match db.write() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.flush() {
                Ok(_) => Response::ok(seq),
                Err(e) => fail(e),
            }
        }
        "compact" => {
            let mut db = match db.write() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            match db.compact() {
                Ok(merged) => {
                    let mut r = Response::ok(seq);
                    r.merged = Some(merged);
                    r
                }
                Err(e) => fail(e),
            }
        }
        "stats" => {
            let db = match db.read() {
                Ok(g) => g,
                Err(_) => return poisoned(),
            };
            let m = db.metrics();
            let mut r = Response::ok(seq);
            r.stats = Some(Stats {
                flushes: m.flushes,
                flushed_records: m.flushed_records,
                flushed_bytes: m.flushed_bytes,
                compactions: m.compactions,
                compacted_records: m.compacted_records,
                tables: db.table_count(),
            });
            r
        }
        other => Response::err(seq, "INVALID_ARGUMENT", format!("unknown op: {other}")),
    }
}

/// Resolve the data directory (creating it) for binaries.
pub fn resolve_dir(arg: Option<String>) -> PathBuf {
    PathBuf::from(arg.unwrap_or_else(|| "./data/fig".to_string()))
}
