//! loadsim: std-only load and store simulator for the Exocortexia suite.
//!
//! Modes:
//!
//!   loadsim http --base http://127.0.0.1:8801 --path /search?q=x \
//!       --concurrency 8 --requests 400 [--p95-budget-ms 250]
//!
//!     Fires `--requests` HTTP/1.1 GETs (Connection: close, one fresh
//!     TcpStream per request) from `--concurrency` threads. Prints one
//!     summary line:
//!       requests=.. errors=.. error_rate=.. p50_ms=.. p95_ms=.. p99_ms=..
//!     Exits 1 if any request errored (connect failure, I/O failure, or a
//!     non-200 status) or if a p95 budget was given and p95 exceeds it.
//!
//!   loadsim store --docs N
//!
//!     Builds `exo_seed::records_corpus(N)` and times: phext serialization,
//!     phext parsing, 200 substring searches for "InterRAI", a 1000-entry
//!     audit-chain build + verify, and 1000 redaction applications. Ends
//!     with a negative control: one character is flipped in a copy of the
//!     audit store and re-verification MUST fail; if it does not, loadsim
//!     prints "NEGATIVE CONTROL FAILED" and exits 1.
//!
//! Any other mode (or no mode) prints usage and exits 2.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::exit;
use std::time::{Duration, Instant};

use exo_core::{AuditLog, Redaction, Store};

fn usage() -> ! {
    eprintln!(
        "usage:\n  loadsim http --base URL --path /x --concurrency C --requests N [--p95-budget-ms B]\n  loadsim store --docs N"
    );
    exit(2);
}

/// Fetch the value following `name` in a hand-rolled `--name value` scan.
fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == name).map(|w| w[1].clone())
}

fn arg_parse<T: std::str::FromStr>(args: &[String], name: &str, default: T) -> T {
    match arg_value(args, name) {
        Some(v) => v.parse().unwrap_or_else(|_| {
            eprintln!("loadsim: bad value for {name}: {v}");
            exit(2);
        }),
        None => default,
    }
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// HTTP mode
// ---------------------------------------------------------------------------

struct BaseUrl {
    host: String,
    port: u16,
    authority: String,
    prefix: String,
}

fn parse_base(base: &str) -> Result<BaseUrl, String> {
    if base.starts_with("https://") {
        return Err("https base URLs are not supported (std-only client, no TLS)".to_string());
    }
    let rest = base
        .strip_prefix("http://")
        .ok_or_else(|| format!("base URL must start with http:// : {base}"))?;
    let (authority, prefix) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, ""),
    };
    if authority.is_empty() {
        return Err(format!("base URL has no host: {base}"));
    }
    let (host, port) = match authority.rfind(':') {
        Some(idx) => {
            let p: u16 = authority[idx + 1..]
                .parse()
                .map_err(|_| format!("bad port in base URL: {base}"))?;
            (&authority[..idx], p)
        }
        None => (authority, 80),
    };
    Ok(BaseUrl {
        host: host.to_string(),
        port,
        authority: authority.to_string(),
        prefix: prefix.to_string(),
    })
}

/// One HTTP/1.1 GET on a fresh connection. Ok(elapsed ms) only for a 200.
fn do_request(base: &BaseUrl, path: &str) -> Result<f64, ()> {
    let start = Instant::now();
    let mut stream = TcpStream::connect((base.host.as_str(), base.port)).map_err(|_| ())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|_| ())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(30)))
        .map_err(|_| ())?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        base.authority
    );
    stream.write_all(request.as_bytes()).map_err(|_| ())?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|_| ())?;
    let elapsed = ms_since(start);

    let text = String::from_utf8_lossy(&raw);
    let status: u16 = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or(())?;
    if status == 200 {
        Ok(elapsed)
    } else {
        Err(())
    }
}

/// Nearest-rank percentile of an ascending-sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted.len() - 1);
    sorted[idx]
}

fn run_http(args: &[String]) -> ! {
    let base_raw = match arg_value(args, "--base") {
        Some(b) => b,
        None => {
            eprintln!("loadsim http: --base is required");
            usage();
        }
    };
    let base = match parse_base(&base_raw) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("loadsim http: {e}");
            exit(2);
        }
    };
    let path_arg = arg_value(args, "--path").unwrap_or_else(|| "/".to_string());
    let path = if path_arg.starts_with('/') {
        format!("{}{}", base.prefix, path_arg)
    } else {
        format!("{}/{}", base.prefix, path_arg)
    };
    let concurrency: usize = arg_parse(args, "--concurrency", 4usize).max(1);
    let requests: usize = arg_parse(args, "--requests", 100usize);
    let budget: Option<f64> = arg_value(args, "--p95-budget-ms").map(|v| {
        v.parse().unwrap_or_else(|_| {
            eprintln!("loadsim http: bad value for --p95-budget-ms: {v}");
            exit(2);
        })
    });

    // Split the request count as evenly as possible across threads.
    let per_thread = requests / concurrency;
    let extra = requests % concurrency;
    let base = std::sync::Arc::new(base);
    let path = std::sync::Arc::new(path);

    let mut handles = Vec::new();
    for t in 0..concurrency {
        let count = per_thread + if t < extra { 1 } else { 0 };
        let base = base.clone();
        let path = path.clone();
        handles.push(std::thread::spawn(move || {
            let mut latencies: Vec<f64> = Vec::with_capacity(count);
            let mut errors: usize = 0;
            for _ in 0..count {
                match do_request(&base, &path) {
                    Ok(ms) => latencies.push(ms),
                    Err(()) => errors += 1,
                }
            }
            (latencies, errors)
        }));
    }

    let mut latencies: Vec<f64> = Vec::new();
    let mut errors: usize = 0;
    for h in handles {
        match h.join() {
            Ok((mut l, e)) => {
                latencies.append(&mut l);
                errors += e;
            }
            Err(_) => errors += 1,
        }
    }

    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&latencies, 50.0);
    let p95 = percentile(&latencies, 95.0);
    let p99 = percentile(&latencies, 99.0);
    let error_rate = if requests == 0 {
        0.0
    } else {
        errors as f64 / requests as f64
    };
    println!(
        "requests={} errors={} error_rate={:.4} p50_ms={:.2} p95_ms={:.2} p99_ms={:.2}",
        requests, errors, error_rate, p50, p95, p99
    );

    if errors > 0 {
        exit(1);
    }
    if let Some(b) = budget {
        if p95 > b {
            eprintln!("loadsim http: p95 {p95:.2} ms exceeds budget {b:.2} ms");
            exit(1);
        }
    }
    exit(0);
}

// ---------------------------------------------------------------------------
// Store mode
// ---------------------------------------------------------------------------

fn run_store(args: &[String]) -> ! {
    let docs: usize = arg_parse(args, "--docs", 10_000usize);
    println!("docs={docs}");

    // Build.
    let t = Instant::now();
    let store = exo_seed::records_corpus(docs);
    println!("build_ms={:.2} stored_docs={}", ms_since(t), store.len());

    // Serialize.
    let t = Instant::now();
    let text = store.to_phext();
    println!("serialize_ms={:.2} bytes={}", ms_since(t), text.len());

    // Parse back.
    let t = Instant::now();
    let parsed = Store::from_phext(&text);
    let parse_ms = ms_since(t);
    println!("parse_ms={:.2} parsed_docs={}", parse_ms, parsed.len());
    if parsed.len() != store.len() {
        eprintln!(
            "loadsim store: round-trip mismatch: built {} docs, parsed {}",
            store.len(),
            parsed.len()
        );
        exit(1);
    }

    // Search: 200 full scans, substring "InterRAI" over every scroll value.
    let coords = store.coords();
    let t = Instant::now();
    let mut hits: usize = 0;
    for _ in 0..200 {
        for coord in &coords {
            if let Some(body) = store.get(coord) {
                if body.contains("InterRAI") {
                    hits += 1;
                }
            }
        }
    }
    println!(
        "search_runs=200 search_hits={} search_ms={:.2}",
        hits,
        ms_since(t)
    );

    // Audit: 1000 appends to a hash-chained log, then verify the chain.
    let mut audit_store = Store::new();
    let mut log = AuditLog::new();
    let t = Instant::now();
    for i in 0..1000u64 {
        let coord = format!("1.1.1/1.1.1/1.1.{}", i + 1);
        let payload = format!("store-benchmark payload {i}");
        log.append(
            &mut audit_store,
            "loadsim",
            "benchmark-append",
            &coord,
            &payload,
        );
    }
    let build_ms = ms_since(t);
    let t = Instant::now();
    let verify_result = log.verify();
    let verify_ms = ms_since(t);
    println!(
        "audit_appends=1000 audit_build_ms={:.2} audit_verify_ms={:.2}",
        build_ms, verify_ms
    );
    if let Err(e) = verify_result {
        eprintln!("loadsim store: audit verify failed: {e}");
        exit(1);
    }
    // Re-open from the store and verify again, exercising AuditLog::open.
    if let Err(e) = AuditLog::open(&audit_store).verify() {
        eprintln!("loadsim store: audit re-open verify failed: {e}");
        exit(1);
    }

    // Redaction: apply one redaction to a sample document 1000 times.
    let sample_coord = coords.first().cloned().unwrap_or_default();
    let sample = store.get(&sample_coord).unwrap_or("").to_string();
    let sample_end = sample.len().min(10);
    let redactions = vec![Redaction {
        target: sample_coord.clone(),
        start: 0,
        end: sample_end,
        category: exo_core::REDACTION_CATEGORIES[0].to_string(),
        reason: "loadsim benchmark redaction".to_string(),
        id: "loadsim-1".to_string(),
    }];
    let t = Instant::now();
    let mut redacted_len: usize = 0;
    for _ in 0..1000 {
        let out = exo_core::apply_redactions(&sample, &redactions);
        redacted_len = out.len();
    }
    println!(
        "redaction_runs=1000 redaction_ms={:.2} redacted_bytes={}",
        ms_since(t),
        redacted_len
    );

    // Negative control: flip exactly one character — the first hex digit of
    // the stored hash in the first audit entry scroll — in a CLONE of the
    // audit store. Re-opening and verifying the clone MUST fail; a pass here
    // would mean the audit chain does not actually detect tampering.
    let mut tampered = audit_store.clone();
    let entry_coord = tampered
        .coords()
        .into_iter()
        .next()
        .expect("audit store has entries");
    let original = tampered
        .get(&entry_coord)
        .expect("audit entry scroll")
        .to_string();
    let hash_start = original.rfind('|').map(|i| i + 1).unwrap_or(0);
    let mut bytes = original.into_bytes();
    if hash_start < bytes.len() {
        bytes[hash_start] = if bytes[hash_start] == b'0' {
            b'1'
        } else {
            b'0'
        };
    }
    let flipped = String::from_utf8(bytes).expect("flip stays valid UTF-8");
    tampered.set(&entry_coord, &flipped);
    match AuditLog::open(&tampered).verify() {
        Ok(()) => {
            println!("NEGATIVE CONTROL FAILED");
            exit(1);
        }
        Err(_) => {
            println!("negative_control=detected");
        }
    }

    exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(|s| s.as_str()) {
        Some("http") => run_http(&args[1..]),
        Some("store") => run_store(&args[1..]),
        _ => usage(),
    }
}
