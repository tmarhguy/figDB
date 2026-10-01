//! `fig-cli`: thin human frontend for the TCP protocol.
//!
//! ```bash
//! fig-cli --addr 127.0.0.1:7001 put k1 v1
//! fig-cli get k1
//! fig-cli scan a z 100
//! fig-cli stats
//! ```
//!
//! Keys/values are CLI strings (raw UTF-8 bytes on the wire). Arbitrary
//! binary keys remain a client-library concern, not a shell concern.

use fig_server::protocol::{self, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

fn usage() -> ! {
    eprintln!("usage: fig-cli [--addr HOST:PORT] <put K V|get K|del K|scan START END [LIMIT]|sync|flush|compact|stats>");
    std::process::exit(2);
}

fn b64(s: &str) -> String {
    protocol::encode_b64(s.as_bytes())
}

fn decode_print(label: &str, v: &Option<String>) {
    match v {
        Some(s) => match protocol::decode_b64(label, s) {
            Ok(bytes) => println!("{}", String::from_utf8_lossy(&bytes)),
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        },
        None => println!("(nil)"),
    }
}

#[tokio::main]
async fn main() {
    let mut argv = std::env::args().skip(1);
    let mut addr = "127.0.0.1:7001".to_string();
    let mut rest: Vec<String> = Vec::new();
    while let Some(a) = argv.next() {
        if a == "--addr" {
            addr = argv.next().unwrap_or_else(|| usage());
        } else {
            rest.push(a);
        }
    }
    if rest.is_empty() {
        usage();
    }
    let (op, body) = match rest[0].as_str() {
        "put" => {
            if rest.len() != 3 {
                usage();
            }
            (
                "put",
                serde_json::json!({"key_b64": b64(&rest[1]), "value_b64": b64(&rest[2])}),
            )
        }
        "get" => {
            if rest.len() != 2 {
                usage();
            }
            ("get", serde_json::json!({"key_b64": b64(&rest[1])}))
        }
        "del" => {
            if rest.len() != 2 {
                usage();
            }
            ("delete", serde_json::json!({"key_b64": b64(&rest[1])}))
        }
        "scan" => {
            if rest.len() < 3 || rest.len() > 4 {
                usage();
            }
            let mut o = serde_json::json!({"start_b64": b64(&rest[1]), "end_b64": b64(&rest[2])});
            if let Some(lim) = rest.get(3) {
                o["limit"] = serde_json::json!(lim.parse::<usize>().unwrap_or_else(|_| usage()));
            }
            ("scan", o)
        }
        "sync" | "flush" | "compact" | "stats" => {
            if rest.len() != 1 {
                usage();
            }
            (rest[0].as_str(), serde_json::json!({}))
        }
        other => {
            eprintln!("unknown subcommand: {other}");
            usage();
        }
    };
    let mut req = serde_json::json!({"seq": 1, "op": op});
    for (k, v) in body.as_object().unwrap() {
        req[k] = v.clone();
    }

    let stream = tokio::net::TcpStream::connect(&addr)
        .await
        .unwrap_or_else(|e| {
            eprintln!("connect {addr}: {e}");
            std::process::exit(1);
        });
    let (r, mut w) = stream.into_split();
    let mut reader = BufReader::new(r);
    let mut line = serde_json::to_string(&req).unwrap();
    line.push('\n');
    w.write_all(line.as_bytes()).await.unwrap();
    w.flush().await.unwrap();
    let mut out = String::new();
    reader.read_line(&mut out).await.unwrap();
    let resp: Response = serde_json::from_str(out.trim_end()).unwrap_or_else(|e| {
        eprintln!("bad response: {e}");
        std::process::exit(1);
    });
    if !resp.ok {
        eprintln!(
            "{}: {}",
            resp.code.unwrap_or_default(),
            resp.message.unwrap_or_default()
        );
        std::process::exit(1);
    }
    match op {
        "put" | "sync" | "flush" => println!("ok"),
        "delete" => println!("{}", resp.removed.unwrap_or(false)),
        "get" => decode_print("value", &resp.value_b64),
        "scan" => {
            for p in resp.pairs.unwrap_or_default() {
                let k = protocol::decode_b64("key", &p.key_b64).unwrap();
                let v = protocol::decode_b64("value", &p.value_b64).unwrap();
                println!(
                    "{}\t{}",
                    String::from_utf8_lossy(&k),
                    String::from_utf8_lossy(&v)
                );
            }
        }
        "compact" => println!("merged={}", resp.merged.unwrap_or(false)),
        "stats" => println!("{}", serde_json::to_string_pretty(&resp.stats).unwrap()),
        _ => unreachable!(),
    }
}
