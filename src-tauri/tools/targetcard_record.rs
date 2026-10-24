//! `targetcard-record` — capture the game's traffic, tag the moments you click
//! a player, and hand the bundle to a collector.
//!
//! Why this exists: finding the target-selection packet needs labelled traffic
//! (a click, then whatever the client sends after it). Shipping files by hand is
//! slow, so this records in the same text format the repo's own tools already
//! read (`share::read_capture`, `a2t-probe opcodes|find|diff`), and uploads it.
//!
//! It does not touch the game. It reads the packets the meter already reads,
//! through the meter's own capture stack (Npcap on Windows, libpcap elsewhere).
//!
//! ```text
//! targetcard-record --seconds 60 --upload http://10.7.1.235:8791 --token TOKEN
//! ```
//!
//! While it runs: click a player, then type their name and press Enter. Type
//! `q` and Enter to stop early.
//!
//! Output, in --out (default `./recordings`):
//!   packets_<stamp>.txt   `<rfc3339>|<stream>|<hex>` per TCP payload
//!   markers.jsonl         one object per marker: index, at_ms, label
//!   manifest.json         host, window, counts, capture availability

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufWriter, Read, Write};
use std::net::{Ipv4Addr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::mpsc;

use a2tools_dps_meter_lib::capture::captured_payload::CapturedPayload;
use a2tools_dps_meter_lib::capture::combat_port_detector::CombatPortDetector;
use a2tools_dps_meter_lib::capture::pcap_capturer::PcapCapturer;
use a2tools_dps_meter_lib::combat::capture_dispatcher::CaptureDispatcher;
use a2tools_dps_meter_lib::combat::data_storage::DataStorage;
use a2tools_dps_meter_lib::combat::ping_tracker::PingTracker;
use a2tools_dps_meter_lib::i18n::lookup::{NpcLookup, SkillLookup};

fn host_name() -> String {
    for key in ["COMPUTERNAME", "HOSTNAME"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return value.trim().to_string();
            }
        }
    }
    "host".to_string()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// RFC3339 with milliseconds, which is what `share::read_capture` parses.
fn rfc3339(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// One capture line: `<rfc3339>|<stream>|<hex payload>`.
fn capture_line(at_ms: i64, stream: &str, data: &[u8]) -> String {
    let mut hex = String::with_capacity(data.len() * 2);
    for byte in data {
        hex.push_str(&format!("{byte:02X}"));
    }
    format!("{}|{}|{}\n", rfc3339(at_ms), stream, hex)
}

fn stream_key(payload: &CapturedPayload) -> String {
    let src = payload.src_ip.unwrap_or(Ipv4Addr::UNSPECIFIED);
    let dst = payload.dst_ip.unwrap_or(Ipv4Addr::UNSPECIFIED);
    format!("{src}:{}>{dst}:{}", payload.src_port, payload.dst_port)
}

struct Args {
    seconds: Option<u64>,
    out: PathBuf,
    upload: Option<String>,
    token: String,
}

fn parse_args() -> Args {
    let mut args = Args {
        seconds: None,
        out: PathBuf::from("recordings"),
        upload: None,
        token: String::new(),
    };
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: usize| argv.get(i + 1).cloned().unwrap_or_default();
        match argv[i].as_str() {
            "--seconds" => {
                args.seconds = next(i).parse().ok();
                i += 2;
            }
            "--out" => {
                args.out = PathBuf::from(next(i));
                i += 2;
            }
            "--upload" => {
                args.upload = Some(next(i));
                i += 2;
            }
            "--token" => {
                args.token = next(i);
                i += 2;
            }
            "--help" | "-h" => {
                println!("targetcard-record [--seconds N] [--out DIR] [--upload URL] [--token T]");
                println!("  --seconds  stop on its own after N seconds (otherwise `q` + Enter)");
                println!("  --upload   collector base url, e.g. http://10.7.1.235:8791");
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument: {other} (try --help)");
                std::process::exit(2);
            }
        }
    }
    args
}

fn http_put(url: &str, token: &str, name: &str, path: &Path) -> Result<(), String> {
    // http://host:port -> PUT http://host:port/upload/<name>. Plain HTTP on the
    // LAN or tailnet, one shared token, no TLS stack inside the tool.
    let rest = url
        .strip_prefix("http://")
        .ok_or("upload url must start with http://")?;
    let (authority, base) = match rest.find('/') {
        Some(idx) => (&rest[..idx], rest[idx..].trim_end_matches('/').to_string()),
        None => (rest, String::new()),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|e| e.to_string())?),
        None => (authority.to_string(), 80),
    };
    let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
    let filename = name.rsplit('/').next().unwrap_or(name).to_string();

    let mut stream =
        TcpStream::connect((host.as_str(), port)).map_err(|e| format!("connect {host}:{port}: {e}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(120))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();

    let head = format!(
        "PUT {base}/upload/{name} HTTP/1.1\r\nHost: {host}\r\nX-Token: {token}\r\n\
         X-Filename: {filename}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes()).map_err(|e| e.to_string())?;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        stream.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    stream.flush().ok();

    let mut reply = String::new();
    stream.read_to_string(&mut reply).map_err(|e| e.to_string())?;
    let status = reply.lines().next().unwrap_or("").to_string();
    if status.contains(" 200") || status.contains(" 201") || status.contains(" 204") {
        Ok(())
    } else {
        Err(format!("server said: {status}"))
    }
}

fn main() {
    let args = parse_args();
    let started = now_ms();

    if let Err(err) = std::fs::create_dir_all(&args.out) {
        eprintln!("cannot create {}: {err}", args.out.display());
        std::process::exit(1);
    }
    let name_stamp = rfc3339(started as i64).replace(':', "").replace('-', "");
    let packets_path = args.out.join(format!("packets_{name_stamp}.txt"));
    let markers_path = args.out.join("markers.jsonl");
    let packets_file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&packets_path)
    {
        Ok(f) => f,
        Err(err) => {
            eprintln!("cannot write {}: {err}", packets_path.display());
            std::process::exit(1);
        }
    };
    let mut markers_file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&markers_path)
    {
        Ok(f) => f,
        Err(err) => {
            eprintln!("cannot write {}: {err}", markers_path.display());
            std::process::exit(1);
        }
    };

    let stop = Arc::new(AtomicBool::new(false));
    let payloads = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));

    // The dispatcher owns port detection and decoding, and without it the
    // capturer never learns which port to filter. It runs exactly as in the app,
    // on a feed of payloads this tool records on the way past.
    let storage = Arc::new(DataStorage::new());
    let port_detector = Arc::new(CombatPortDetector::new());
    let (forward_tx, forward_rx) = mpsc::channel::<CapturedPayload>(4096);
    {
        let dispatcher = CaptureDispatcher::new(
            Arc::clone(&storage),
            Arc::new(SkillLookup::new()),
            Arc::new(NpcLookup::new()),
            Arc::clone(&port_detector),
            Arc::new(PingTracker::new()),
        );
        std::thread::spawn(move || match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(runtime) => runtime.block_on(dispatcher.run(forward_rx)),
            Err(err) => eprintln!("decoder runtime: {err}"),
        });
    }

    // Markers come from stdin, so recording never waits on the user.
    let marker_handle = {
        let stop = Arc::clone(&stop);
        let storage = Arc::clone(&storage);
        std::thread::spawn(move || {
            let stdin = std::io::stdin();
            let mut index = 0usize;
            for line in stdin.lock().lines() {
                let Ok(line) = line else { break };
                let label = line.trim().to_string();
                if label.eq_ignore_ascii_case("q") || label.eq_ignore_ascii_case("quit") {
                    stop.store(true, Ordering::Relaxed);
                    break;
                }
                index += 1;
                let at = now_ms();
                // The name table at the click is what makes this data usable:
                // it turns "I clicked Sins" into an entity id to hunt for in the
                // packets that follow.
                let names: std::collections::BTreeMap<i32, String> =
                    storage.get_nicknames().into_iter().collect();
                let record = serde_json::json!({
                    "index": index,
                    "at_ms": at,
                    "label": label,
                    "names": names,
                });
                let _ = writeln!(markers_file, "{record}");
                let _ = markers_file.flush();
                println!("marker {index} at {} {}", rfc3339(at as i64), if label.is_empty() { "" } else { label.as_str() });
            }
        })
    };

    // The meter's own capture stack: Npcap loaded at runtime, port detection,
    // TCP reassembly input as `CapturedPayload`.
    let (tx, mut rx) = mpsc::channel::<CapturedPayload>(4096);
    let capturer = PcapCapturer::new(tx, Arc::clone(&port_detector));
    let capture_available = a2tools_dps_meter_lib::platform::pcap::library_available();
    if capture_available {
        capturer.start();
    }

    println!("recording to {}", packets_path.display());
    if !capture_available {
        println!("WARNING: no packet capture library (Npcap on Windows). Nothing will be recorded.");
    }
    println!("click a player, then type their name and press Enter. `q` + Enter stops.\n");

    let deadline = args.seconds.map(|s| started + s * 1000);
    let mut out = BufWriter::with_capacity(1 << 20, packets_file);
    while !stop.load(Ordering::Relaxed) {
        if let Some(deadline) = deadline {
            if now_ms() >= deadline {
                break;
            }
        }
        // A short wait keeps the loop responsive to `q` without busy-spinning.
        match rx.try_recv() {
            Ok(payload) => {
                let line = capture_line(payload.captured_at_ms, &stream_key(&payload), &payload.data);
                if out.write_all(line.as_bytes()).is_ok() {
                    payloads.fetch_add(1, Ordering::Relaxed);
                    bytes.fetch_add(payload.data.len() as u64, Ordering::Relaxed);
                }
                // Let the decoder see it too, so port detection and the name
                // table keep working while we record.
                let _ = forward_tx.try_send(payload);
            }
            Err(mpsc::error::TryRecvError::Empty) => std::thread::sleep(Duration::from_millis(5)),
            Err(mpsc::error::TryRecvError::Disconnected) => break,
        }
    }

    capturer.stop();
    let _ = out.flush();
    drop(out);
    let ended = now_ms();

    let manifest = serde_json::json!({
        "tool": "targetcard-record",
        "version": env!("CARGO_PKG_VERSION"),
        "started_ms": started,
        "ended_ms": ended,
        "started_at": rfc3339(started as i64),
        "capture_available": capture_available,
        "payloads": payloads.load(Ordering::Relaxed),
        "payload_bytes": bytes.load(Ordering::Relaxed),
        "platform": std::env::consts::OS,
        "host": host_name(),
    });
    let manifest_path = args.out.join("manifest.json");
    let _ = std::fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    );
    drop(marker_handle);

    let size = std::fs::metadata(&packets_path).map(|m| m.len()).unwrap_or(0);
    println!(
        "\n{} payloads, {:.1} MB, {:.0} s",
        payloads.load(Ordering::Relaxed),
        size as f64 / 1_048_576.0,
        (ended.saturating_sub(started)) as f64 / 1000.0
    );

    match args.upload {
        Some(url) if capture_available => {
            let host = manifest["host"].as_str().unwrap_or("host").to_string();
            let session = format!("{host}-{}", name_stamp.replace(['T', 'Z'], "").replace('.', ""));
            let mut failures = 0;
            for file in [&packets_path, &markers_path, &manifest_path] {
                if std::fs::metadata(file).map(|m| m.len()).unwrap_or(0) == 0 {
                    println!("skipped empty {}", file.display());
                    continue;
                }
                let file_name = file
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                let remote = format!("{session}/{file_name}");
                match http_put(&url, &args.token, &remote, file) {
                    Ok(()) => println!("uploaded {remote}"),
                    Err(err) => {
                        failures += 1;
                        println!("upload failed for {remote}: {err}");
                    }
                }
            }
            if failures == 0 {
                println!("session {session} is on the collector");
            } else {
                println!("kept locally in {}", args.out.display());
            }
        }
        Some(_) => println!("skipped upload: nothing was captured"),
        None => println!("no --upload given; files are in {}", args.out.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_line_is_something_the_repo_can_read_back() {
        // The whole point of this format: `share::read_capture` (and so
        // `a2t-probe`) must parse what this tool writes.
        let dir = std::env::temp_dir().join("targetcard-record-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("packets_test.txt");
        std::fs::write(
            &path,
            capture_line(1_750_000_000_123, "10.0.0.2:5000>10.0.0.9:443", &[0x36, 0x44, 0x01]),
        )
        .unwrap();

        let packets = a2tools_dps_meter_lib::share::read_capture(&path).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].stream, "10.0.0.2:5000>10.0.0.9:443");
        assert_eq!(packets[0].bytes, vec![0x36, 0x44, 0x01]);
        assert_eq!(packets[0].captured_at_ms, 1_750_000_000_123);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn http_put_sends_what_the_collector_expects() {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;

        // A throwaway listener, so the request this tool really sends is checked
        // rather than described.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(sock.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                headers.push(line.trim_end().to_string());
            }
            let mut body = vec![0u8; 5];
            reader.read_exact(&mut body).unwrap();
            let _ = sock.write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\nok");
            (request_line, headers, body)
        });

        let dir = std::env::temp_dir().join("targetcard-record-put");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("packets.txt");
        std::fs::write(&file, b"hello").unwrap();

        let result = http_put(&format!("http://{addr}"), "tok", "sess/packets.txt", &file);
        assert!(result.is_ok(), "{result:?}");

        let (line, headers, body) = server.join().unwrap();
        assert!(line.starts_with("PUT /upload/sess/packets.txt "), "{line}");
        assert!(headers.iter().any(|h| h == "X-Token: tok"), "{headers:?}");
        assert!(headers.iter().any(|h| h == "Content-Length: 5"), "{headers:?}");
        assert_eq!(body, b"hello");
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn the_stream_key_separates_directions() {
        let mut there = CapturedPayload {
            src_port: 5000,
            dst_port: 443,
            data: Vec::new(),
            device_name: None,
            captured_at_ms: 0,
            src_ip: Some(Ipv4Addr::new(10, 0, 0, 2)),
            dst_ip: Some(Ipv4Addr::new(10, 0, 0, 9)),
            tcp_seq: 0,
            tcp_ack: 0,
        };
        assert_eq!(stream_key(&there), "10.0.0.2:5000>10.0.0.9:443");
        there.src_port = 443;
        there.dst_port = 5000;
        assert_eq!(stream_key(&there), "10.0.0.2:443>10.0.0.9:5000");
    }
}
