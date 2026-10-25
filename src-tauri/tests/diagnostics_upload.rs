//! Pushes one gzipped log slice at a real collector, so the upload path is
//! proven against the thing it actually talks to rather than a stub.
//!
//! `TC_COLLECTOR_URL` and `TC_COLLECTOR_TOKEN`; skips when either is unset, so
//! CI and a fresh clone stay offline.

use a2tools_dps_meter_lib::diagnostics::{gzip, send_once, CollectorConfig};

#[tokio::test]
async fn a_real_collector_takes_a_log_slice() {
    let (Ok(url), Ok(token)) = (
        std::env::var("TC_COLLECTOR_URL"),
        std::env::var("TC_COLLECTOR_TOKEN"),
    ) else {
        return;
    };

    let config = CollectorConfig {
        upload_url: url,
        token,
        interval_secs: 300,
    };
    let body = gzip(b"targetcard diagnostics selftest\n").expect("gzip");
    send_once(&config, body, "selftest/1-debug.log.gz")
        .await
        .expect("the collector should accept a log slice");
}
