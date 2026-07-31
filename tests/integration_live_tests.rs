//! Port of `tests/test_gfs_dataset.py::TestIntegration` (Python) — real
//! network calls against NOMADS/NOAA. Skipped by default; opt in with:
//!
//! ```bash
//! cargo test -- --ignored
//! ```
//!
//! **Data-freshness constraint**: NOMADS only keeps a rolling few days of
//! GFS history (empirically more, but treat 3 days back as the safe
//! ceiling). Every test below resolves its date from "now" (never a
//! hardcoded past date), exactly like [`noawclg::auto_date`] is meant to be
//! used in real code — see the "GFS weather forecasts" section of the
//! crate README for the same guidance applied to library usage examples.

use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use noawclg::http::{Fetcher, ReqwestFetcher};
use noawclg::{GfsDatasetManager, Region};

/// `noawclg`'s `download_hours` doesn't expose a bare "reachability" check,
/// so — like the Python `test_nomads_reachable` — this talks to NOMADS
/// directly through the same [`Fetcher`] abstraction the rest of the crate
/// uses (rather than pulling in a second HTTP client just for this probe).
#[test]
#[ignore = "hits the real network; run with `cargo test -- --ignored`"]
fn nomads_reachable() {
    let fetcher = ReqwestFetcher::new(Duration::from_secs(15)).unwrap();
    let (status, _) = fetcher.get("https://nomads.ncep.noaa.gov").unwrap();
    assert!(status < 500, "NOMADS returned unexpected status {status}");
}

#[test]
#[ignore = "hits the real network; run with `cargo test -- --ignored`"]
fn filter_endpoint_reachable() {
    let fetcher = ReqwestFetcher::new(Duration::from_secs(15)).unwrap();
    let (status, _) = fetcher
        .get("https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25_1hr.pl")
        .unwrap();
    // 400/404 are fine (no query params supplied); 5xx means the server itself is down.
    assert!(status < 500, "grib-filter endpoint returned {status}");
}

#[test]
#[ignore = "hits the real network; run with `cargo test -- --ignored`"]
fn downloads_a_single_real_hour_within_the_freshness_window() {
    // Yesterday's 00Z run is always published by now — same reasoning as
    // `auto_date(1)`, just with a fixed cycle so this test isn't sensitive
    // to what hour it happens to run at.
    let yesterday = (Utc::now() - ChronoDuration::days(1))
        .format("%Y%m%d")
        .to_string();

    let dir = tempfile::tempdir().unwrap();
    let mgr = GfsDatasetManager::with_options(
        &yesterday,
        "00",
        dir.path(),
        Some(Region {
            toplat: 5.0,
            bottomlat: -35.0,
            leftlon: -75.0,
            rightlon: -34.0,
        }),
        Duration::from_secs(2),
        Duration::from_secs(30),
    )
    .unwrap();

    let files = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    let Some(path) = files.get(&0) else {
        eprintln!("NOMADS returned nothing for {yesterday} 00Z f000 — service unavailable, skipping assertion");
        return;
    };
    let size = std::fs::metadata(path).unwrap().len();
    assert!(
        size > 1024,
        "downloaded file suspiciously small ({size} bytes)"
    );
}
