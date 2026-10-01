//! `fig-bench`: honest load generator for the TCP server.
//!
//! Single connection, sequential requests — matches the serialized server,
//! so each op latency is a true round-trip number, not a queueing artifact.
//!
//! ```bash
//! fig-bench --addr 127.0.0.1:7001 --n 20000 --value-size 64 --sync-every 500
//! fig-bench --addr 127.0.0.1:7001 --n 20000 --prefix soak-03- --verify-only
//! ```
//!
//! Keys `PREFIX + {i:08}`, values deterministic from `(prefix, i)` so any
//! later `--verify-only` run recomputes the oracle with zero shared state.

use fig_server::protocol::{self, Response};
use std::time::Instant;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn usage() -> ! {
    eprintln!(
        "usage: fig-bench [--addr HOST:PORT] [--n N] [--value-size BYTES] [--sync-every K] [--prefix P] [--verify-only] [--compact]"
    );
    std::process::exit(2);
}

fn arg_val(args: &mut std::vec::IntoIter<String>, flag: &str) -> String {
    args.next().unwrap_or_else(|| {
        eprintln!("{flag} needs a value");
        std::process::exit(2);
    })
}

/// Deterministic value bytes from (prefix, i): xorshift stream, full byte range.
fn value(prefix: &str, i: usize, size: usize) -> Vec<u8> {
    let mut h = 0x9E37_79B9_7F4A_7C15u64;
    for b in prefix.bytes() {
        h = h.wrapping_mul(0x1000_0000_01B3).wrapping_add(b as u64);
    }
    h = h.wrapping_add(i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut state = h.max(1);
    let mut out = Vec::with_capacity(size);
    while out.len() < size {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(size);
    out
}

fn key(prefix: &str, i: usize) -> Vec<u8> {
    format!("{prefix}{i:08}").into_bytes()
}

struct Conn {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
    seq: u64,
}

impl Conn {
    async fn connect(addr: &str) -> Self {
        let stream = tokio::net::TcpStream::connect(addr)
            .await
            .unwrap_or_else(|e| {
                eprintln!("connect {addr}: {e}");
                std::process::exit(1);
            });
        let (r, w) = stream.into_split();
        Self {
            reader: BufReader::new(r),
            writer: w,
            seq: 0,
        }
    }

    async fn call(&mut self, op: &str, body: serde_json::Value) -> Response {
        self.seq += 1;
        let mut req = serde_json::json!({"seq": self.seq, "op": op});
        for (k, v) in body.as_object().unwrap() {
            req[k] = v.clone();
        }
        let mut line = serde_json::to_string(&req).unwrap();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).await.unwrap();
        self.writer.flush().await.unwrap();
        let mut out = String::new();
        self.reader.read_line(&mut out).await.unwrap();
        serde_json::from_str(out.trim_end()).unwrap_or_else(|e| {
            eprintln!("bad response: {e}");
            std::process::exit(1);
        })
    }

    /// Timed call; returns (response, latency_micros).
    async fn timed(&mut self, op: &str, body: serde_json::Value) -> (Response, u64) {
        let t0 = Instant::now();
        let r = self.call(op, body).await;
        (r, t0.elapsed().as_micros() as u64)
    }
}

fn percentile(sorted: &[u64], pct: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((pct / 100.0) * sorted.len() as f64).ceil() as usize - 1;
    sorted[idx.min(sorted.len() - 1)]
}

fn report(name: &str, lat: &[u64]) {
    let mut s = lat.to_vec();
    s.sort_unstable();
    let sum: u128 = s.iter().map(|x| *x as u128).sum();
    let secs = sum as f64 / 1_000_000.0;
    println!(
        "{name}: n={} {:.0}/s avg={:.2}ms p50={:.2}ms p99={:.2}ms max={:.2}ms",
        s.len(),
        s.len() as f64 / secs.max(1e-9),
        sum as f64 / s.len().max(1) as f64 / 1000.0,
        percentile(&s, 50.0) as f64 / 1000.0,
        percentile(&s, 99.0) as f64 / 1000.0,
        *s.last().unwrap_or(&0) as f64 / 1000.0,
    );
}

#[tokio::main]
async fn main() {
    let mut argv = std::env::args().skip(1).collect::<Vec<_>>().into_iter();
    let mut addr = "127.0.0.1:7001".to_string();
    let mut n: usize = 20_000;
    let mut value_size: usize = 64;
    let mut sync_every: usize = 500;
    let mut prefix = "bench-".to_string();
    let mut verify_only = false;
    let mut compact = false;
    while let Some(a) = argv.next() {
        match a.as_str() {
            "--addr" => addr = arg_val(&mut argv, "--addr"),
            "--n" => {
                n = arg_val(&mut argv, "--n")
                    .parse()
                    .unwrap_or_else(|_| usage())
            }
            "--value-size" => {
                value_size = arg_val(&mut argv, "--value-size")
                    .parse()
                    .unwrap_or_else(|_| usage());
            }
            "--sync-every" => {
                sync_every = arg_val(&mut argv, "--sync-every")
                    .parse()
                    .unwrap_or_else(|_| usage());
            }
            "--prefix" => prefix = arg_val(&mut argv, "--prefix"),
            "--verify-only" => verify_only = true,
            "--compact" => compact = true,
            "--help" | "-h" => {
                println!("usage: fig-bench [--addr HOST:PORT] [--n N] [--value-size BYTES] [--sync-every K] [--prefix P] [--verify-only] [--compact]");
                return;
            }
            _ => usage(),
        }
    }

    let mut c = Conn::connect(&addr).await;
    if !verify_only {
        // Write phase: puts are acknowledged only at sync boundaries.
        let mut lat = Vec::with_capacity(n);
        for i in 0..n {
            let body = serde_json::json!({
                "key_b64": protocol::encode_b64(&key(&prefix, i)),
                "value_b64": protocol::encode_b64(&value(&prefix, i, value_size)),
            });
            let (r, us) = c.timed("put", body).await;
            if !r.ok {
                eprintln!("put failed: {r:?}");
                std::process::exit(1);
            }
            lat.push(us);
            if sync_every > 0 && (i + 1) % sync_every == 0 {
                let r = c.call("sync", serde_json::json!({})).await;
                if !r.ok {
                    eprintln!("sync failed: {r:?}");
                    std::process::exit(1);
                }
            }
        }
        let r = c.call("sync", serde_json::json!({})).await;
        assert!(r.ok, "final sync failed: {r:?}");
        report(
            &format!("PUT value_size={value_size} sync_every={sync_every}"),
            &lat,
        );
    }

    // Read/verify phase: every key must match the recomputed oracle.
    let mut lat = Vec::with_capacity(n);
    let mut state = 0x0123_4567_89ab_cdefu64;
    for _ in 0..n {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let i = (state % n as u64) as usize;
        let body = serde_json::json!({"key_b64": protocol::encode_b64(&key(&prefix, i))});
        let (r, us) = c.timed("get", body).await;
        if !r.ok {
            eprintln!("get {prefix}{i:08} failed: {r:?}");
            std::process::exit(1);
        }
        let got = protocol::decode_b64("value", &r.value_b64.unwrap()).unwrap();
        assert_eq!(got, value(&prefix, i, value_size), "value mismatch at {i}");
        lat.push(us);
    }
    report("GET random", &lat);

    if compact {
        let t0 = Instant::now();
        let r = c.call("compact", serde_json::json!({})).await;
        println!(
            "COMPACT merged={} in {:.2}s",
            r.merged.unwrap_or(false),
            t0.elapsed().as_secs_f64()
        );
    }
    let r = c.call("stats", serde_json::json!({})).await;
    println!("STATS {}", serde_json::to_string(&r.stats).unwrap());
    println!("PASS: fig-bench n={n} prefix={prefix}");
}
