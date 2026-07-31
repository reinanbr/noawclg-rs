//! Port of `tests/test_gfs_dataset.py::TestZarr` (Python) — the Rust crate's
//! Zarr path is a real, self-contained writer/reader (not mocked the way
//! the Python tests have to mock `xr.Dataset.to_zarr`/`.chunk()` to dodge a
//! Dask dependency), so this is a genuine on-disk round trip through the
//! public API only.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use ndarray::{ArrayD, IxDyn};
use noawclg::gfs_dataset::{GfsDataset, GfsVariable};
use noawclg::persistence;

fn sample_dataset() -> GfsDataset {
    let time = vec![
        Utc.with_ymd_and_hms(2026, 4, 3, 6, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 4, 3, 12, 0, 0).unwrap(),
    ];
    let lat: Vec<f64> = (0..8).map(|i| -5.0 + i as f64 * (10.0 / 7.0)).collect();
    let lon: Vec<f64> = (0..8).map(|i| -75.0 + i as f64).collect();
    let data = ArrayD::from_shape_vec(
        IxDyn(&[2, lat.len(), lon.len()]),
        (0..2 * lat.len() * lon.len())
            .map(|x| x as f64 * 0.1)
            .collect(),
    )
    .unwrap();

    let mut variables = HashMap::new();
    variables.insert(
        "t2m".to_string(),
        GfsVariable {
            data,
            dims: vec!["time".into(), "latitude".into(), "longitude".into()],
            long_name: "2 metre temperature".into(),
            units: "C".into(),
        },
    );

    let mut attrs = HashMap::new();
    attrs.insert("run_date".to_string(), "20260403".to_string());

    GfsDataset {
        time,
        forecast_hour: vec![6, 12],
        latitude: lat,
        longitude: lon,
        level: None,
        var_order: vec!["t2m".to_string()],
        variables,
        attrs,
    }
}

#[test]
fn save_zarr_creates_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("o.zarr");
    let path = persistence::save_zarr(&sample_dataset(), &store).unwrap();
    assert!(path.is_dir());
    assert!(path.join(".zgroup").exists());
}

#[test]
fn zarr_round_trip_preserves_values_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("rt.zarr");
    let ds = sample_dataset();

    persistence::save_zarr(&ds, &store).unwrap();
    let loaded = persistence::load_zarr(&store).unwrap();

    assert_eq!(loaded.time.len(), ds.time.len());
    assert_eq!(loaded.latitude, ds.latitude);
    assert_eq!(loaded.longitude, ds.longitude);
    assert_eq!(loaded.forecast_hour, ds.forecast_hour);

    let orig = &ds.variables["t2m"];
    let round = &loaded.variables["t2m"];
    assert_eq!(orig.data.shape(), round.data.shape());
    for (a, b) in orig.data.iter().zip(round.data.iter()) {
        assert!((a - b).abs() < 1e-9);
    }
    assert_eq!(round.long_name, "2 metre temperature");
    assert_eq!(round.units, "C");
}

#[test]
fn zarr_relative_store_path_still_resolves() {
    // save_zarr/load_zarr accept any AsRef<Path>; a nested-but-nonexistent
    // parent must be created automatically (mirrors output_dir creation).
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("nested").join("deeper.zarr");
    persistence::save_zarr(&sample_dataset(), &store).unwrap();
    assert!(store.exists());
}

#[cfg(not(feature = "netcdf-io"))]
#[test]
fn netcdf_without_feature_is_feature_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let err = persistence::save_netcdf(&sample_dataset(), dir.path().join("o.nc")).unwrap_err();
    assert!(matches!(err, noawclg::Error::FeatureDisabled("netcdf-io")));
}
