//! Single-node TCP server: one `Database` behind a mutex, one task per
//! connection, line-delimited JSON per `protocol`.
//!
//! Concurrency model (deliberate): requests execute one at a time under the
//! lock. The engine is single-threaded today — foreground flush/compaction
//! included — so serialization is correctness, not a shortcut. Concurrent
//! clients are accepted (each gets a task) but their requests interleave
//! safely. A background compaction / lock-striping design arrives when writes
//! become concurrent.

use crate::protocol::{
    self, Pair, Request, Response, Stats, DEFAULT_SCAN_LIMIT, MAX_LINE_BYTES, MAX_SCAN_LIMIT,
};
use fig_core::{Config, Error};
use fig_storage::Database;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;

/// Open (or create) the database at `dir` with server defaults and serve
/// `listener` forever. `flush_threshold` bounds the memtable before a
/// foreground flush; fsync policy is `Never` — durability is explicit via
/// the `sync` op and periodic client syncs, exactly like the library API.
pub async fn serve(
    listener: TcpListener,
    dir: &Path,
    flush_threshold: usize,
) -> anyhow::Result<()> {
    let db = open_db(dir, flush_threshold)?;
    let db = Arc::new(Mutex::new(db));
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
pub async fn handle_conn(stream: TcpStream, db: Arc<Mutex<Database>>) -> anyhow::Result<()> {
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
        let resp = dispatch(&db, line.trim_end_matches(['\r', '\n'])).await;
        let mut out = serde_json::to_vec(&resp)?;
        out.push(b'\n');
        writer.write_all(&out).await?;
        writer.flush().await?;
    }
}

async fn dispatch(db: &Arc<Mutex<Database>>, line: &str) -> Response {
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

    let mut db = db.lock().await;
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
        "sync" => match db.sync() {
            Ok(()) => Response::ok(seq),
            Err(e) => fail(e),
        },
        "flush" => match db.flush() {
            Ok(_) => Response::ok(seq),
            Err(e) => fail(e),
        },
        "compact" => match db.compact() {
            Ok(merged) => {
                let mut r = Response::ok(seq);
                r.merged = Some(merged);
                r
            }
            Err(e) => fail(e),
        },
        "stats" => {
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
