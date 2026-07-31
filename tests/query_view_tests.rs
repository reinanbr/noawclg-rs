//! Port of `tests/test_main.py` (Python) — `TestHelpers`, `TestDatasetView`,
//! `TestBoundingBox`, `TestGetNoaaData` (point/time-series selection; the
//! network-backed dataset build itself is replaced by
//! `GetNoaaData::from_dataset`, exactly like the Python tests replace
//! `GFSDatasetManager` with a `MagicMock`), and `TestLoadFunction`.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use ndarray::{ArrayD, IxDyn};
use noawclg::coords::{find_dim, normalize_lon, parse_date};
#[cfg(not(feature = "grib"))]
use noawclg::Error;
use noawclg::{BoundingBox, GetNoaaData, GfsDataset, GfsVariable};

fn sample_dataset(var_name: &str) -> GfsDataset {
    let time = vec![
        Utc.with_ymd_and_hms(2026, 4, 3, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 4, 3, 3, 0, 0).unwrap(),
    ];
    let data =
        ArrayD::from_shape_vec(IxDyn(&[2, 3, 3]), (0..18).map(|x| x as f64).collect()).unwrap();

    let mut variables = HashMap::new();
    variables.insert(
        var_name.to_string(),
        GfsVariable {
            data,
            dims: vec!["time".into(), "latitude".into(), "longitude".into()],
            long_name: "2 metre temperature".into(),
            units: "C".into(),
        },
    );

    GfsDataset {
        time,
        forecast_hour: vec![0, 3],
        latitude: vec![-4.0, -3.0, -2.0],
        longitude: vec![320.0, 321.0, 322.0],
        level: None,
        var_order: vec![var_name.to_string()],
        variables,
        attrs: HashMap::new(),
    }
}

// ── TestHelpers ─────────────────────────────────────────────────────────

#[test]
fn parse_date_br_format() {
    assert_eq!(parse_date("17/04/2026").unwrap(), "20260417");
}

#[test]
fn normalize_lon_for_minus180_to_180() {
    assert!((normalize_lon(-38.5, -179.5) - (-38.5)).abs() < 1e-9);
}

#[test]
fn normalize_lon_for_0_to_360() {
    assert!((normalize_lon(-38.5, 0.0) - 321.5).abs() < 1e-9);
}

#[test]
fn find_dim_detects_existing() {
    let coords = vec![
        "step".to_string(),
        "latitude".to_string(),
        "longitude".to_string(),
    ];
    let found = find_dim(&coords, &["lat", "latitude"], "lat").unwrap();
    assert_eq!(found, "latitude");
}

#[test]
fn find_dim_raises_with_helpful_message() {
    let coords = vec!["x".to_string(), "y".to_string()];
    let err = find_dim(&coords, &["lat", "latitude"], "lat").unwrap_err();
    assert!(err.to_string().contains("Cannot find a lat coordinate"));
}

// ── TestBoundingBox ─────────────────────────────────────────────────────

#[test]
fn bounding_box_contains() {
    let b = BoundingBox::new(-10.0, 10.0, -80.0, -30.0);
    assert!(b.contains(-3.7, -38.5));
    assert!(!b.contains(20.0, -38.5));
}

// ── TestGetNoaaData (via from_dataset, no network) ─────────────────────

#[test]
fn get_keys_returns_long_names() {
    let noaa = GetNoaaData::from_dataset(
        "20260403".into(),
        "00".into(),
        vec!["t2m".into()],
        vec![0, 3],
        sample_dataset("t2m"),
    );
    let keys = noaa.get_keys();
    assert_eq!(keys["t2m"], "2 metre temperature");
}

#[test]
fn get_data_from_point_selects_nearest() {
    let noaa = GetNoaaData::from_dataset(
        "20260403".into(),
        "00".into(),
        vec!["t2m".into()],
        vec![0, 3],
        sample_dataset("t2m"),
    );
    let view = noaa.get_data_from_point((-3.1, -38.5), None).unwrap();
    // -38.5 normalized into [0, 360) is 321.5, exactly between grid points
    // 321.0 and 322.0; matches xarray's observed nearest-neighbour tie-break.
    assert!((view.longitude - 322.0).abs() < 1e-9);
}

#[test]
fn get_data_from_point_missing_variable_lookup_errors() {
    let noaa = GetNoaaData::from_dataset(
        "20260403".into(),
        "00".into(),
        vec!["t2m".into()],
        vec![0, 3],
        sample_dataset("t2m"),
    );
    let view = noaa.get_data_from_point((-3.1, -38.5), None).unwrap();
    let err = view.get("prate").unwrap_err();
    assert!(err.to_string().contains("Variable 'prate' not found"));
}

#[test]
fn get_time_series_variable_and_missing() {
    let noaa = GetNoaaData::from_dataset(
        "20260403".into(),
        "00".into(),
        vec!["t2m".into()],
        vec![0, 3],
        sample_dataset("t2m"),
    );

    let view = noaa.get_time_series((-3.1, -38.5), Some("t2m")).unwrap();
    assert!(view.get("t2m").is_ok());

    let err = noaa
        .get_time_series((-3.1, -38.5), Some("prate"))
        .unwrap_err();
    assert!(err.to_string().contains("Variable 'prate' not found"));
}

#[test]
fn dataset_view_to_table_has_one_row_per_time_step() {
    let noaa = GetNoaaData::from_dataset(
        "20260403".into(),
        "00".into(),
        vec!["t2m".into()],
        vec![0, 3],
        sample_dataset("t2m"),
    );
    let view = noaa.get_data_from_point((-3.1, -38.5), None).unwrap();
    let table = view.to_table();
    assert_eq!(table.len(), 2);
    assert!(table[0].contains_key("t2m"));
}

// ── TestLoadFunction ────────────────────────────────────────────────────
// `load()` needs the `grib` feature to actually decode anything; without
// it, it must fail deterministically (no network attempted) rather than
// panicking or hanging.

#[cfg(not(feature = "grib"))]
#[test]
fn load_without_grib_feature_is_feature_disabled() {
    let err = noawclg::load(
        "03/04/2026",
        "00",
        vec!["t2m".to_string()],
        vec![0, 3],
        None,
    )
    .unwrap_err();
    assert!(matches!(err, Error::FeatureDisabled("grib")));
}
