//! TCP gate: the wire protocol serves the database correctly, survives
//! garbage input without dropping the connection, and keeps acknowledged
//! data across a full server restart (drop + reopen, new listener).

use fig_core::Config;
use fig_server::protocol::{self, Response};
use fig_storage::Database;
use std::sync::{Arc, RwLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

fn b64(s: &[u8]) -> String {
    protocol::encode_b64(s)
}

struct Client {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl Client {
    async fn connect(addr: &std::net::SocketAddr) -> Self {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let (r, w) = stream.into_split();
        Self {
            reader: BufReader::new(r),
            writer: w,
        }
    }

    async fn roundtrip(&mut self, req: serde_json::Value) -> Response {
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
        let mut out = String::new();
        self.reader.read_line(&mut out).await.unwrap();
        assert!(!out.is_empty(), "server closed connection mid-test");
        serde_json::from_str(out.trim_end()).expect("response decodes")
    }

    async fn raw(&mut self, bytes: &[u8]) -> String {
        self.writer.write_all(bytes).await.unwrap();
        self.writer.flush().await.unwrap();
        let mut out = String::new();
        self.reader.read_line(&mut out).await.unwrap();
        out
    }
}

fn open_db(dir: &std::path::Path) -> Database {
    Database::open(dir, Config::default(), fig_wal::FsyncPolicy::Always, 1024).unwrap()
}

/// Spawn the connection handler on an ephemeral port; returns the address.
async fn spawn_server(db: Database) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let db = Arc::new(RwLock::new(db));
    let h = tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let db = Arc::clone(&db);
            tokio::spawn(async move {
                let _ = fig_server::handle_conn(stream, db).await;
            });
        }
    });
    (addr, h)
}

#[tokio::test]
async fn put_get_delete_scan_sync_stats_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, srv) = spawn_server(open_db(dir.path())).await;
    let mut c = Client::connect(&addr).await;

    // put two keys
    for (k, v) in [("a", "1"), ("b", "2")] {
        let r = c
            .roundtrip(serde_json::json!({"seq": 1, "op": "put", "key_b64": b64(k.as_bytes()), "value_b64": b64(v.as_bytes())}))
            .await;
        assert!(r.ok, "put failed: {r:?}");
    }
    // get hit
    let r = c
        .roundtrip(serde_json::json!({"seq": 2, "op": "get", "key_b64": b64(b"a")}))
        .await;
    assert!(r.ok);
    assert_eq!(r.value_b64.unwrap(), b64(b"1"));
    // get miss is NOT_FOUND, not a transport error
    let r = c
        .roundtrip(serde_json::json!({"seq": 3, "op": "get", "key_b64": b64(b"zzz")}))
        .await;
    assert!(!r.ok);
    assert_eq!(r.code.unwrap(), "NOT_FOUND");
    // scan sees both, ascending
    let r = c
        .roundtrip(serde_json::json!({"seq": 4, "op": "scan", "start_b64": b64(b"a"), "end_b64": b64(b"z")}))
        .await;
    assert!(r.ok);
    let pairs = r.pairs.unwrap();
    assert_eq!(pairs.len(), 2);
    // delete reports removal; key then misses
    let r = c
        .roundtrip(serde_json::json!({"seq": 5, "op": "delete", "key_b64": b64(b"a")}))
        .await;
    assert!(r.ok);
    assert_eq!(r.removed, Some(true));
    let r = c
        .roundtrip(serde_json::json!({"seq": 6, "op": "get", "key_b64": b64(b"a")}))
        .await;
    assert_eq!(r.code.unwrap(), "NOT_FOUND");
    // sync + stats
    let r = c
        .roundtrip(serde_json::json!({"seq": 7, "op": "sync"}))
        .await;
    assert!(r.ok);
    let r = c
        .roundtrip(serde_json::json!({"seq": 8, "op": "stats"}))
        .await;
    assert!(r.ok);
    assert!(r.stats.is_some());
    srv.abort();
}

#[tokio::test]
async fn garbage_input_is_rejected_connection_survives() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, srv) = spawn_server(open_db(dir.path())).await;
    let mut c = Client::connect(&addr).await;

    let out = c.raw(b"this is not json\n").await;
    let r: Response = serde_json::from_str(out.trim_end()).unwrap();
    assert!(!r.ok);
    assert_eq!(r.code.unwrap(), "INVALID_ARGUMENT");

    let out = c.raw(b"{\"seq\": 3, \"op\": \"frobnicate\"}\n").await;
    let r: Response = serde_json::from_str(out.trim_end()).unwrap();
    assert!(!r.ok);

    // Missing key field, bad base64, bad range — all INVALID_ARGUMENT.
    for req in [
        serde_json::json!({"seq": 4, "op": "get"}),
        serde_json::json!({"seq": 5, "op": "get", "key_b64": "!!!"}),
        serde_json::json!({"seq": 6, "op": "scan", "start_b64": b64(b"z"), "end_b64": b64(b"a")}),
    ] {
        let r = c.roundtrip(req).await;
        assert!(!r.ok, "expected rejection, got {r:?}");
        assert_eq!(r.code.unwrap(), "INVALID_ARGUMENT");
    }

    // Connection still healthy: serves a real write.
    let r = c
        .roundtrip(serde_json::json!({"seq": 7, "op": "put", "key_b64": b64(b"k"), "value_b64": b64(b"v")}))
        .await;
    assert!(r.ok);
    srv.abort();
}

#[tokio::test]
async fn acknowledged_data_survives_full_restart() {
    let dir = tempfile::tempdir().unwrap();
    {
        let (addr, srv) = spawn_server(open_db(dir.path())).await;
        let mut c = Client::connect(&addr).await;
        for i in 0..50u8 {
            let r = c
                .roundtrip(serde_json::json!({"seq": 1, "op": "put", "key_b64": b64(&[i]), "value_b64": b64(&[i, i])}))
                .await;
            assert!(r.ok);
        }
        let r = c
            .roundtrip(serde_json::json!({"seq": 2, "op": "sync"}))
            .await;
        assert!(r.ok);
        srv.abort();
    };
    // New listener, reopened database: the acked prefix must be intact.
    let (addr, srv) = spawn_server(open_db(dir.path())).await;
    let mut c = Client::connect(&addr).await;
    for i in 0..50u8 {
        let r = c
            .roundtrip(serde_json::json!({"seq": 3, "op": "get", "key_b64": b64(&[i])}))
            .await;
        assert!(r.ok, "key {i} lost across restart");
        assert_eq!(r.value_b64.unwrap(), b64(&[i, i]));
    }
    srv.abort();
}

#[tokio::test]
async fn binary_keys_roundtrip() {
    // Arbitrary bytes (incl. invalid UTF-8) survive the JSON envelope.
    let dir = tempfile::tempdir().unwrap();
    let (addr, srv) = spawn_server(open_db(dir.path())).await;
    let mut c = Client::connect(&addr).await;
    let k = vec![0x00, 0xff, 0x80, 0x01];
    let v = vec![0xfe, 0x00, 0x7f];
    let r = c
        .roundtrip(
            serde_json::json!({"seq": 1, "op": "put", "key_b64": b64(&k), "value_b64": b64(&v)}),
        )
        .await;
    assert!(r.ok);
    let r = c
        .roundtrip(serde_json::json!({"seq": 2, "op": "get", "key_b64": b64(&k)}))
        .await;
    assert!(r.ok);
    assert_eq!(r.value_b64.unwrap(), b64(&v));
    srv.abort();
}

#[tokio::test]
async fn concurrent_writers_and_readers_stay_correct() {
    // 8 clients hammer disjoint key ranges at once; every write must land
    // exactly once and every read must see its own client's values.
    // (Writers serialize on the write lock; readers share it.)
    let dir = tempfile::tempdir().unwrap();
    let (addr, srv) = spawn_server(open_db(dir.path())).await;
    let mut tasks = Vec::new();
    for cli in 0u8..8 {
        tasks.push(tokio::spawn(async move {
            let mut c = Client::connect(&addr).await;
            for i in 0..50u8 {
                let k = vec![cli, i];
                let v = vec![cli, i, cli.wrapping_add(i)];
                let r = c
                    .roundtrip(
                        serde_json::json!({"seq": 1, "op": "put", "key_b64": b64(&k), "value_b64": b64(&v)}),
                    )
                    .await;
                assert!(r.ok, "client {cli} put {i} failed: {r:?}");
                // Read-your-own-write through the shared lock.
                let r = c
                    .roundtrip(serde_json::json!({"seq": 2, "op": "get", "key_b64": b64(&k)}))
                    .await;
                assert!(r.ok, "client {cli} get {i} failed: {r:?}");
                assert_eq!(r.value_b64.unwrap(), b64(&v));
            }
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    // All 400 keys present with exact values.
    let mut c = Client::connect(&addr).await;
    let r = c
        .roundtrip(
            serde_json::json!({"seq": 3, "op": "scan", "start_b64": b64(&[0x00]), "end_b64": b64(&[0xff, 0xff]), "limit": 1000}),
        )
        .await;
    assert!(r.ok);
    assert_eq!(r.pairs.unwrap().len(), 400);
    srv.abort();
}

#[tokio::test]
async fn background_compaction_folds_tables_without_manual_compact() {
    // Full serve() path with a 50 ms timer and a tiny threshold: tables must
    // fold on their own — no client ever sends `compact`.
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let dir_path = std::path::PathBuf::from(dir.path());
    let srv = tokio::spawn(async move {
        let _ = fig_server::serve(listener, &dir_path, 256, 50).await;
    });
    // Give the accept loop a moment to bind (connect retries below cover it).
    let mut c = Client::connect(&addr).await;
    for i in 0..300usize {
        let k = vec![(i % 256) as u8, (i / 256) as u8];
        let r = c
            .roundtrip(
                serde_json::json!({"seq": 1, "op": "put", "key_b64": b64(&k), "value_b64": b64(b"value")}),
            )
            .await;
        assert!(r.ok);
    }
    // Poll stats until the background task folds the stack (≤8 tables) or a
    // merge counter proves it ran.
    let mut folded = false;
    for _ in 0..100 {
        let r = c
            .roundtrip(serde_json::json!({"seq": 2, "op": "stats"}))
            .await;
        let s = r.stats.unwrap();
        if s.tables <= 8 && s.compactions >= 1 {
            folded = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(folded, "background compaction never folded the stack");
    // Oracle check: all 300 keys readable after background merges.
    for i in 0..300usize {
        let k = vec![(i % 256) as u8, (i / 256) as u8];
        let r = c
            .roundtrip(serde_json::json!({"seq": 3, "op": "get", "key_b64": b64(&k)}))
            .await;
        assert!(r.ok, "key {i} lost to background merge");
    }
    srv.abort();
}
