//! Port of `tests/test_gfs_dataset.py` (Python) — `TestInit`, `TestHelpers`
//! (via captured request URLs and returned cache paths, since the
//! corresponding Rust methods are private — the same information the
//! Python tests check by calling `mgr._region_params()` etc.),
//! `TestDownloadHours`, and `TestEdgeCases`. Network I/O is replaced by
//! `common::FakeFetcher`, never a real socket.

mod common;

use std::time::Duration;

use common::{dummy_grib_bytes, Canned, FakeFetcher};
use noawclg::{BoundingBox, Error, GfsDatasetManager, Region};

const BRAZIL: Region = Region {
    toplat: 5.0,
    bottomlat: -35.0,
    leftlon: -75.0,
    rightlon: -34.0,
};

fn mgr_with(
    fetcher: FakeFetcher,
    region: Option<Region>,
) -> (GfsDatasetManager, tempfile::TempDir, common::CallLog) {
    let dir = tempfile::tempdir().unwrap();
    let log = fetcher.log();
    let mgr = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        region,
        Duration::from_millis(0),
        Duration::from_secs(5),
        Box::new(fetcher),
    )
    .unwrap();
    (mgr, dir, log)
}

// ── TestInit ─────────────────────────────────────────────────────────────

#[test]
fn valid_construction_stores_attrs() {
    let (mgr, dir, _log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    assert_eq!(mgr.date, "20260403");
    assert_eq!(mgr.cycle, "06");
    assert_eq!(mgr.output_dir, dir.path());
}

#[test]
fn output_dir_created_automatically() {
    let base = tempfile::tempdir().unwrap();
    let sub = base.path().join("new").join("nested");
    let fetcher = FakeFetcher::always(Canned::status(404));
    GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        &sub,
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(fetcher),
    )
    .unwrap();
    assert!(sub.exists());
}

#[test]
fn invalid_date_format_raises() {
    let dir = tempfile::tempdir().unwrap();
    let err = GfsDatasetManager::with_fetcher(
        "20260432",
        "06",
        dir.path(),
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(FakeFetcher::always(Canned::status(404))),
    )
    .unwrap_err();
    assert!(matches!(err, Error::InvalidDate(_)));
}

#[test]
fn invalid_cycle_raises_with_message() {
    let dir = tempfile::tempdir().unwrap();
    let err = GfsDatasetManager::with_fetcher(
        "20260403",
        "03",
        dir.path(),
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(FakeFetcher::always(Canned::status(404))),
    )
    .unwrap_err();
    assert!(err.to_string().contains("cycle must be one of"));
}

#[test]
fn all_valid_cycles_accepted() {
    for cycle in ["00", "06", "12", "18"] {
        let dir = tempfile::tempdir().unwrap();
        let mgr = GfsDatasetManager::with_fetcher(
            "20260403",
            cycle,
            dir.path(),
            None,
            Duration::ZERO,
            Duration::from_secs(5),
            Box::new(FakeFetcher::always(Canned::status(404))),
        )
        .unwrap();
        assert_eq!(mgr.cycle, cycle);
    }
}

#[test]
fn region_stored() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::status(404)), Some(BRAZIL));
    assert_eq!(mgr.region, Some(BRAZIL));
}

#[test]
fn none_region_stored() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    assert_eq!(mgr.region, None);
}

#[test]
fn custom_pause_stored() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        None,
        Duration::from_secs(5),
        Duration::from_secs(5),
        Box::new(FakeFetcher::always(Canned::status(404))),
    )
    .unwrap();
    assert_eq!(mgr.pause, Duration::from_secs(5));
}

// ── TestHelpers (region/var/filter-url params, cache path shape) ──────────
// The Python tests call private methods (`_region_params`, `_var_params`,
// `_filter_url`, `_cache_path`) directly; here those are private too, so we
// observe the same information through the URL `download_hours` actually
// issues (captured by `FakeFetcher`) and the `PathBuf`s it returns.

#[test]
fn filter_url_contains_region_params_when_set() {
    let (mgr, _dir, log) = mgr_with(FakeFetcher::always(Canned::status(404)), Some(BRAZIL));
    let _ = mgr.download_hours(&["t2m"], &[0], false);
    let url = &log.urls()[0];
    assert!(url.contains("&subregion=&toplat=5&bottomlat=-35&leftlon=-75&rightlon=-34"));
}

#[test]
fn filter_url_excludes_subregion_for_global() {
    let (mgr, _dir, log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let _ = mgr.download_hours(&["t2m"], &[0], false);
    assert!(!log.urls()[0].contains("subregion"));
}

#[test]
fn filter_url_embeds_date_cycle_and_zero_padded_hour() {
    let (mgr, _dir, log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let _ = mgr.download_hours(&["t2m"], &[6], false);
    let url = &log.urls()[0];
    assert!(url.contains("20260403"));
    assert!(url.contains("t06z"));
    assert!(url.contains("f006"));
}

#[test]
fn var_params_contains_grib_tokens() {
    let (mgr, _dir, log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let _ = mgr.download_hours(&["t2m"], &[0], false);
    let url = &log.urls()[0];
    assert!(url.contains("var_TMP=on"));
    assert!(url.contains("lev_2_m_above_ground=on"));
}

#[test]
fn var_params_multiple_vars_both_present() {
    let (mgr, _dir, log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let _ = mgr.download_hours(&["t2m", "prate"], &[0], false);
    let url = &log.urls()[0];
    assert!(url.contains("var_TMP=on"));
    assert!(url.contains("var_PRATE=on"));
}

#[test]
fn var_params_shared_level_token_deduplicated() {
    // u10 and v10 share "lev_10_m_above_ground" but have distinct var tokens.
    let (mgr, _dir, log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let _ = mgr.download_hours(&["u10", "v10"], &[0], false);
    let url = &log.urls()[0];
    assert!(url.contains("var_UGRD=on"));
    assert!(url.contains("var_VGRD=on"));
    assert_eq!(url.matches("lev_10_m_above_ground=on").count(), 1);
}

#[test]
fn cache_path_is_absolute_with_grib2_extension() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::ok(dummy_grib_bytes(200))), None);
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    let path = &result[&0];
    assert!(path.is_absolute());
    assert_eq!(path.extension().unwrap(), "grib2");
}

#[test]
fn cache_path_contains_date_cycle_hour() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::ok(dummy_grib_bytes(200))), None);
    let result = mgr.download_hours(&["t2m"], &[24], false).unwrap();
    let name = result[&24]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(name.contains("20260403"));
    assert!(name.contains("06z"));
    assert!(name.contains("f024"));
}

#[test]
fn cache_path_global_tag_says_global() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::ok(dummy_grib_bytes(200))), None);
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert!(result[&0]
        .file_name()
        .unwrap()
        .to_string_lossy()
        .contains("global"));
}

#[test]
fn cache_path_order_independent() {
    let (mgr, dir, _log) = mgr_with(FakeFetcher::always(Canned::ok(dummy_grib_bytes(200))), None);
    let r1 = mgr.download_hours(&["t2m", "prate"], &[0], false).unwrap();
    // fresh manager sharing the same output_dir + a fresh cache-hit fetcher
    let (mgr2, _dir2, _log2) = {
        let fetcher = FakeFetcher::always(Canned::status(500)); // must NOT be called (cache hit)
        let log = fetcher.log();
        let m = GfsDatasetManager::with_fetcher(
            "20260403",
            "06",
            dir.path(),
            None,
            Duration::ZERO,
            Duration::from_secs(5),
            Box::new(fetcher),
        )
        .unwrap();
        (m, dir, log)
    };
    let r2 = mgr2.download_hours(&["prate", "t2m"], &[0], false).unwrap();
    assert_eq!(r1[&0], r2[&0]);
}

// ── TestDownloadHours ───────────────────────────────────────────────────

#[test]
fn unknown_var_raises() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let err = mgr.download_hours(&["not_a_var"], &[0], false).unwrap_err();
    assert!(matches!(err, Error::UnknownVariables(_)));
}

#[test]
fn cache_hit_skips_network() {
    let dir = tempfile::tempdir().unwrap();
    let fetcher = FakeFetcher::always(Canned::ok(dummy_grib_bytes(200)));
    let log = fetcher.log();
    let mgr = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(fetcher),
    )
    .unwrap();

    let first = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert_eq!(log.count(), 1);

    let second = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert_eq!(log.count(), 1, "second call must be a pure cache hit");
    assert_eq!(first[&0], second[&0]);
}

#[test]
fn force_bypasses_cache() {
    let dir = tempfile::tempdir().unwrap();
    let fetcher = FakeFetcher::always(Canned::ok(dummy_grib_bytes(200)));
    let log = fetcher.log();
    let mgr = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(fetcher),
    )
    .unwrap();

    mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert_eq!(log.count(), 1);
    mgr.download_hours(&["t2m"], &[0], true).unwrap();
    assert_eq!(log.count(), 2, "force=true must re-issue the request");
}

#[test]
fn success_returns_existing_file() {
    let (mgr, _dir, _log) = mgr_with(
        FakeFetcher::always(Canned::ok(dummy_grib_bytes(1024))),
        None,
    );
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert!(result[&0].exists());
}

#[test]
fn http_error_omits_hour() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::status(404)), None);
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert!(!result.contains_key(&0));
}

#[test]
fn tiny_response_omits_hour() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::always(Canned::ok(b"tiny".to_vec())), None);
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert!(!result.contains_key(&0));
}

#[test]
fn multiple_hours_all_present() {
    let (mgr, _dir, _log) = mgr_with(
        FakeFetcher::always(Canned::ok(dummy_grib_bytes(1024))),
        None,
    );
    let result = mgr.download_hours(&["t2m"], &[0, 6, 12], false).unwrap();
    let mut hours: Vec<u32> = result.keys().copied().collect();
    hours.sort_unstable();
    assert_eq!(hours, vec![0, 6, 12]);
}

#[test]
fn all_cached_no_network_calls() {
    let dir = tempfile::tempdir().unwrap();
    let warm_fetcher = FakeFetcher::always(Canned::ok(dummy_grib_bytes(200)));
    let warm_mgr = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(warm_fetcher),
    )
    .unwrap();
    warm_mgr.download_hours(&["t2m"], &[0, 6], false).unwrap();

    let cold_fetcher = FakeFetcher::always(Canned::status(500));
    let cold_log = cold_fetcher.log();
    let cold_mgr = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        None,
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(cold_fetcher),
    )
    .unwrap();
    let result = cold_mgr.download_hours(&["t2m"], &[0, 6], false).unwrap();
    assert_eq!(cold_log.count(), 0);
    let mut hours: Vec<u32> = result.keys().copied().collect();
    hours.sort_unstable();
    assert_eq!(hours, vec![0, 6]);
}

#[test]
fn network_exception_omits_hour() {
    let (mgr, _dir, _log) = mgr_with(FakeFetcher::failing(), None);
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert!(!result.contains_key(&0));
}

// ── TestEdgeCases ───────────────────────────────────────────────────────

#[test]
fn two_regions_no_cache_collision() {
    let dir = tempfile::tempdir().unwrap();
    let region_b = Region {
        toplat: 10.0,
        bottomlat: -10.0,
        leftlon: -60.0,
        rightlon: -40.0,
    };
    let a = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        Some(BRAZIL),
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(FakeFetcher::always(Canned::ok(dummy_grib_bytes(200)))),
    )
    .unwrap();
    let b = GfsDatasetManager::with_fetcher(
        "20260403",
        "06",
        dir.path(),
        Some(region_b),
        Duration::ZERO,
        Duration::from_secs(5),
        Box::new(FakeFetcher::always(Canned::ok(dummy_grib_bytes(200)))),
    )
    .unwrap();
    let pa = a.download_hours(&["t2m"], &[0], false).unwrap();
    let pb = b.download_hours(&["t2m"], &[0], false).unwrap();
    assert_ne!(pa[&0], pb[&0]);
}

#[test]
fn bounding_box_still_works_standalone() {
    // Sanity check that the crate's other public types remain usable
    // alongside GfsDatasetManager in the same integration binary.
    let b = BoundingBox::new(-10.0, 10.0, -80.0, -30.0);
    assert!(b.contains(-3.7, -38.5));
}

#[test]
fn retries_transient_server_error_then_succeeds() {
    // First attempt hits a transient 503, the retry succeeds — mirrors
    // NOMADS occasionally 503'ing under load.
    let (mgr, _dir, log) = mgr_with(
        FakeFetcher::queue(vec![Canned::status(503), Canned::ok(dummy_grib_bytes(200))]),
        None,
    );
    let result = mgr.download_hours(&["t2m"], &[0], false).unwrap();
    assert!(result.contains_key(&0));
    assert_eq!(log.count(), 2, "expected one retry after the 503");
}
