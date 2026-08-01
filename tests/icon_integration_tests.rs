//! Real network + real `cdo` calls against DWD's ICON global open-data
//! feed. Skipped by default; opt in with:
//!
//! ```bash
//! cargo test --features icon -- --ignored
//! ```
//!
//! Same data-freshness reasoning as `integration_live_tests.rs`'s GFS
//! tests: resolve the date from "now", never a hardcoded past date.
//!
//! The whole file is gated on the `icon` feature so this test *binary*
//! still compiles (as a no-op) in the default/`grib`-only CI jobs, which
//! don't have `netcdf`/`bzip2`/`tar` in the dependency graph at all.
#![cfg(feature = "icon")]

use std::time::Duration;

use chrono::Utc;
use noawclg::icon_dataset::IconDatasetManager;

#[test]
#[ignore = "hits the real network and shells out to cdo; run with `cargo test --features icon -- --ignored`"]
fn dwd_icon_open_data_reachable() {
    use noawclg::http::{Fetcher, ReqwestFetcher};
    let fetcher = ReqwestFetcher::new(Duration::from_secs(15)).unwrap();
    let (status, _) = fetcher
        .get("https://opendata.dwd.de/weather/nwp/icon/grib/00/t_2m/")
        .unwrap();
    assert!(
        status < 500,
        "DWD open-data returned unexpected status {status}"
    );
}

#[test]
#[ignore = "hits the real network and shells out to cdo; run with `cargo test --features icon -- --ignored`"]
fn builds_a_real_t2m_dataset_with_plausible_values() {
    let today = Utc::now().format("%Y%m%d").to_string();
    let dir = tempfile::tempdir().unwrap();

    let mgr = IconDatasetManager::with_options(&today, "00", dir.path(), Duration::from_secs(120))
        .unwrap();

    let ds = mgr
        .build_dataset(&["t2m"], &[0, 3])
        .expect("ICON build_dataset should succeed against the live 00Z run");

    assert_eq!(ds.forecast_hour.len(), 2, "expected both requested hours");
    assert_eq!(ds.latitude.len(), 721);
    assert_eq!(ds.longitude.len(), 1440);

    let t2m = ds.get("t2m").expect("t2m variable present");
    assert_eq!(t2m.data.shape(), &[2, 721, 1440]);

    // Sanity check: every decoded Celsius value should be in a physically
    // plausible range for 2 m air temperature anywhere on Earth. This is
    // the real end-to-end assertion that download -> bz2 decompress -> cdo
    // decode -> nearest-neighbor remap -> unit conversion produced sane
    // data, not garbage from a wrong byte offset or a misapplied gather.
    let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in t2m.data.iter() {
        min = min.min(v);
        max = max.max(v);
    }
    assert!(
        min > -90.0 && max < 60.0,
        "t2m out of plausible range: min={min} max={max}"
    );

    // Sao Paulo, Brazil (this app's default location) in August (southern
    // hemisphere winter) should read as a mild, non-extreme temperature —
    // a coarse but meaningful cross-check that the remap landed values at
    // the right physical location, not just "some" plausible value.
    let lat_idx = ds
        .latitude
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (**a - (-23.55))
                .abs()
                .partial_cmp(&(**b - (-23.55)).abs())
                .unwrap()
        })
        .unwrap()
        .0;
    let lon_idx = ds
        .longitude
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (**a - (360.0 - 46.63))
                .abs()
                .partial_cmp(&(**b - (360.0 - 46.63)).abs())
                .unwrap()
        })
        .unwrap()
        .0;
    let sp_temp = t2m.data[[0, lat_idx, lon_idx]];
    assert!(
        (0.0..40.0).contains(&sp_temp),
        "Sao Paulo t2m implausible for August: {sp_temp}C"
    );
}

#[test]
#[ignore = "hits the real network and shells out to cdo; run with `cargo test --features icon -- --ignored`"]
fn precip_rate_is_derived_and_nonnegative() {
    let today = Utc::now().format("%Y%m%d").to_string();
    let dir = tempfile::tempdir().unwrap();
    let mgr = IconDatasetManager::with_options(&today, "00", dir.path(), Duration::from_secs(120))
        .unwrap();

    let ds = mgr
        .build_dataset(&["prate"], &[0, 6, 12])
        .expect("ICON build_dataset should succeed for prate");

    let prate = ds.get("prate").expect("prate derived from tot_prec");
    for &v in prate.data.iter() {
        assert!(
            v >= 0.0,
            "derived precip rate should never be negative, got {v}"
        );
        assert!(v < 500.0, "derived precip rate implausibly high: {v} mm/h");
    }
}

#[test]
#[ignore = "hits the real network and shells out to cdo; run with `cargo test --features icon -- --ignored`"]
fn multi_variable_dataset_shares_one_hour_axis_including_gust_fallback_at_hour_zero() {
    let today = Utc::now().format("%Y%m%d").to_string();
    let dir = tempfile::tempdir().unwrap();
    let mgr = IconDatasetManager::with_options(&today, "00", dir.path(), Duration::from_secs(120))
        .unwrap();

    // VMAX_10M (gust's source field) isn't published at hour 0 — this
    // exercises the sustained-wind-speed fallback in `fill_gaps`, in the
    // same multi-variable build real backend requests use.
    let ds = mgr
        .build_dataset(
            &["t2m", "u10", "v10", "gust", "prmsl", "tcc", "cape"],
            &[0, 6],
        )
        .expect("multi-variable ICON build_dataset should succeed");

    assert_eq!(
        ds.forecast_hour,
        vec![0, 6],
        "every variable must share this exact axis"
    );
    for key in ["t2m", "u10", "v10", "gust", "prmsl", "tcc", "cape"] {
        let v = ds
            .get(key)
            .unwrap_or_else(|| panic!("missing variable '{key}'"));
        assert_eq!(
            v.data.shape()[0],
            2,
            "'{key}' hour axis doesn't match the shared axis"
        );
        assert!(
            v.data.iter().all(|x| x.is_finite()),
            "'{key}' contains a non-finite value (NaN/inf would break JSON serialization downstream)"
        );
    }

    // At hour 0, gust must equal the sustained-wind fallback exactly (no
    // VMAX_10M available yet), not zero or some other placeholder.
    let gust = &ds.get("gust").unwrap().data;
    let u10 = &ds.get("u10").unwrap().data;
    let v10 = &ds.get("v10").unwrap().data;
    for i in 0..ds.latitude.len() {
        for j in 0..ds.longitude.len() {
            let expected = u10[[0, i, j]].hypot(v10[[0, i, j]]);
            assert!(
                (gust[[0, i, j]] - expected).abs() < 1e-9,
                "hour-0 gust should fall back to sustained wind speed"
            );
        }
    }
}
