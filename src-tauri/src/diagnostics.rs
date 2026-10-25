//! Optional: push this app's debug log to a collector on the player's own
//! network, so a problem can be read instead of described.
//!
//! Off unless `diagnostics.json` exists in the app data directory, which means a
//! normal install sends nothing and the app has no upload code path it can take
//! by accident:
//!
//! ```json
//! { "upload_url": "http://10.7.1.235:8791", "token": "…", "interval_secs": 300 }
//! ```
//!
//! The build script writes that file when given `-CollectorToken`, so the token
//! stays out of the repository. Only the tail of `debug.log` is sent: a log that
//! grows for hours is not more useful than its last megabyte, and gzip makes the
//! difference between a few hundred kilobytes and a few megabytes per upload.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How much of the log tail is sent. One megabyte is roughly the last few
/// thousand lines, far more than any single session's worth of a problem.
const MAX_TAIL_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CollectorConfig {
    pub upload_url: String,
    pub token: String,
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
}

fn default_interval() -> u64 {
    300
}

pub fn config_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("diagnostics.json")
}

/// The collector configuration, if this install has one.
pub fn load(app_data_dir: &Path) -> Option<CollectorConfig> {
    let raw = std::fs::read_to_string(config_path(app_data_dir)).ok()?;
    // Windows editors and PowerShell's Set-Content can leave a byte-order mark,
    // which JSON does not allow and serde will not skip.
    let raw = raw.trim_start_matches('\u{feff}');
    match serde_json::from_str::<CollectorConfig>(&raw) {
        Ok(config) if !config.upload_url.trim().is_empty() && !config.token.trim().is_empty() => {
            Some(config)
        }
        Ok(_) => {
            tracing::warn!("diagnostics: diagnostics.json is missing upload_url or token; uploads off");
            None
        }
        Err(error) => {
            tracing::warn!("diagnostics: diagnostics.json is not valid JSON ({error}); uploads off");
            None
        }
    }
}

/// The last `limit` bytes of a file.
pub fn tail(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(length.saturating_sub(limit))).ok()?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).ok()?;
    Some(buffer)
}

pub fn gzip(data: &[u8]) -> Option<Vec<u8>> {
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(data).ok()?;
    encoder.finish().ok()
}

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

fn stamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

/// PUT one gzipped slice of the log. `name` is `<session>/<file>`.
pub async fn send_once(config: &CollectorConfig, body: Vec<u8>, name: &str) -> Result<(), String> {
    let url = format!(
        "{}/upload/{}",
        config.upload_url.trim_end_matches('/'),
        name
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())?;
    let response = client
        .put(url)
        .header("X-Token", config.token.trim())
        .header(reqwest::header::CONTENT_TYPE, "application/gzip")
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!("collector answered {}", response.status()))
    }
}

/// Start the watcher, if this install has a collector configured.
///
/// Returns whether it started, so the caller can say so in the log either way.
pub fn start(app_data_dir: PathBuf, log_path: PathBuf) -> bool {
    let Some(config) = load(&app_data_dir) else {
        return false;
    };
    tracing::info!(
        "diagnostics: sending {} to {} every {}s",
        log_path.display(),
        config.upload_url,
        config.interval_secs
    );
    tokio::spawn(async move {
        let session = format!("{}-logs", host_name());
        let mut last_sent: Option<(u64, u64)> = None; // (len, mtime secs)
        loop {
            tokio::time::sleep(Duration::from_secs(config.interval_secs.max(30))).await;

            let Ok(meta) = std::fs::metadata(&log_path) else {
                continue;
            };
            if meta.len() == 0 {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if last_sent == Some((meta.len(), mtime)) {
                continue; // nothing written since the last upload
            }

            let Some(tail) = tail(&log_path, MAX_TAIL_BYTES) else {
                continue;
            };
            let Some(zipped) = gzip(&tail) else {
                continue;
            };
            let name = format!("{session}/{}-debug.log.gz", stamp());
            match send_once(&config, zipped, &name).await {
                Ok(()) => {
                    last_sent = Some((meta.len(), mtime));
                    tracing::debug!("diagnostics: sent {name}");
                }
                Err(error) => {
                    // Retried on the next tick: the log will have grown, so the
                    // whole point of the next upload is this one plus more.
                    tracing::warn!("diagnostics: upload failed: {error}");
                }
            }
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("targetcard-diag-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_tail_is_the_end_of_the_file() {
        let dir = temp_dir("tail");
        let path = dir.join("debug.log");
        std::fs::write(&path, "0123456789").unwrap();
        assert_eq!(tail(&path, 4).unwrap(), b"6789");
        // A file shorter than the limit comes back whole.
        assert_eq!(tail(&path, 99).unwrap(), b"0123456789");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gzip_round_trips() {
        let text = b"targetcard: PDRC@Phernos search found no character".repeat(20);
        let zipped = gzip(&text).unwrap();
        assert!(zipped.len() < text.len() / 2, "gzip should shrink a log tail");
        assert_eq!(&zipped[..2], &[0x1f, 0x8b]);
    }

    #[test]
    fn a_config_with_a_byte_order_mark_still_loads() {
        let dir = temp_dir("bom");
        std::fs::write(
            config_path(&dir),
            format!("\u{feff}{{\"upload_url\":\"http://x\",\"token\":\"t\"}}"),
        )
        .unwrap();
        assert!(load(&dir).is_some(), "a BOM must not disable uploads");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_partial_config_disables_uploads() {
        let dir = temp_dir("config");
        assert!(load(&dir).is_none());

        std::fs::write(config_path(&dir), r#"{"upload_url":"http://x","token":""}"#).unwrap();
        assert!(load(&dir).is_none(), "an empty token must not enable uploads");

        std::fs::write(
            config_path(&dir),
            r#"{"upload_url":"http://10.0.0.5:8791","token":"t"}"#,
        )
        .unwrap();
        let config = load(&dir).expect("a complete config loads");
        assert_eq!(config.interval_secs, 300, "the interval has a default");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_upload_is_a_put_with_the_token_and_the_body() {
        use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (read, mut write) = socket.split();
            let mut reader = tokio::io::BufReader::new(read);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).await.unwrap();
            let mut headers = Vec::new();
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                headers.push(line.trim_end().to_string());
            }
            // Read exactly Content-Length: waiting for EOF would deadlock, since
            // the client keeps the connection open until it has its answer.
            let length: usize = headers
                .iter()
                .find_map(|h| {
                    let (name, value) = h.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())?
                })
                .unwrap_or(0);
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).await.unwrap();
            write
                .write_all(b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
            (request_line, headers, body)
        });

        let config = CollectorConfig {
            upload_url: format!("http://{addr}"),
            token: "secret".to_string(),
            interval_secs: 300,
        };
        send_once(&config, b"body-bytes".to_vec(), "PIRATE-logs/1-debug.log.gz")
            .await
            .expect("upload should succeed");

        let (line, headers, body) = server.await.unwrap();
        assert!(line.starts_with("PUT /upload/PIRATE-logs/1-debug.log.gz "), "{line}");
        assert!(headers.iter().any(|h| h == "x-token: secret" || h == "X-Token: secret"), "{headers:?}");
        assert_eq!(body, b"body-bytes");
    }
}
