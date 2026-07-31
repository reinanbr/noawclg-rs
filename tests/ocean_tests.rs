//! Port of `tests/test_ocean.py` (Python), exercised through `noawclg`'s
//! public API only — this also double-checks that every type needed to use
//! the ocean/ENSO API is actually re-exported from the crate root.

use chrono::NaiveDate;
use ndarray::{Array3, Array4};
use noawclg::{
    classify_enso, enso_summary_from_series, nino_anomaly_from_series, oni_from_series,
    thermocline_depth_from_field4, warm_water_volume_from_field4, Field3, Field4, GodasVarMeta,
    TimeSeries, GODAS_LEVELS, GODAS_VARS, NINO_BOXES,
};

fn monthly_index(start_year: i32, n: usize) -> Vec<NaiveDate> {
    (0..n)
        .map(|i| {
            NaiveDate::from_ymd_opt(start_year + (i as i32) / 12, (i as u32 % 12) + 1, 1).unwrap()
        })
        .collect()
}

// ── GODAS_VARS / NINO_BOXES catalogues ─────────────────────────────────

#[test]
fn godas_vars_has_five_variables() {
    let mut keys: Vec<&str> = GODAS_VARS.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["pottmp", "salt", "sshg", "ucur", "vcur"]);
}

#[test]
fn godas_var_meta_is_publicly_constructible_type() {
    let meta: GodasVarMeta = GODAS_VARS["pottmp"];
    assert!(meta.has_levels);
    assert_eq!(meta.units_out, "°C");
}

#[test]
fn nino_boxes_has_four_regions() {
    let mut keys: Vec<&str> = NINO_BOXES.keys().copied().collect();
    keys.sort_unstable();
    assert_eq!(keys, vec!["1+2", "3", "3.4", "4"]);
}

#[test]
fn godas_levels_has_40_entries() {
    assert_eq!(GODAS_LEVELS.len(), 40);
    assert_eq!(GODAS_LEVELS[0], 5.0);
    assert_eq!(*GODAS_LEVELS.last().unwrap(), 4478.0);
}

// ── ENSO index math (pure — no network required) ────────────────────────

fn synthetic_sst(mean: f64, amplitude: f64, n: usize) -> TimeSeries {
    let index = monthly_index(2015, n);
    let values: Vec<f64> = (0..n)
        .map(|i| mean + amplitude * (4.0 * std::f64::consts::PI * i as f64 / n as f64).sin())
        .collect();
    TimeSeries::new(index, values, "SST_Nino34")
}

#[test]
fn nino_anomaly_mean_near_zero_during_climatology() {
    let sst = synthetic_sst(27.0, 0.3, 72);
    let anom = nino_anomaly_from_series(&sst, 2015, 2019, 2015, 2019);
    assert!(anom.mean().abs() < 0.05);
}

#[test]
fn oni_has_expected_length_and_name() {
    // sst spans 72 months (2015-01..2020-12); requesting only 2015-2019
    // (60 months) must filter the output down to that target range.
    let sst = synthetic_sst(27.0, 0.3, 72);
    let oni = oni_from_series(&sst, 2015, 2019, 2015, 2019);
    assert_eq!(oni.name, "ONI");
    assert_eq!(oni.len(), 60);
}

#[test]
fn classify_enso_detects_a_sustained_warm_event() {
    let n = 36;
    let index = monthly_index(2015, n);
    let mut values = vec![0.0; n];
    for v in values.iter_mut().take(14).skip(6) {
        *v = 0.8;
    }
    let oni = TimeSeries::new(index, values, "ONI");
    let phase = classify_enso(&oni, 0.5, 5);
    assert!(phase.contains(&"El Niño".to_string()));
}

#[test]
fn enso_summary_rows_have_valid_phases() {
    let sst = synthetic_sst(27.0, 0.3, 72);
    let rows = enso_summary_from_series(&sst, 2015, 2017, 2015, 2019);
    assert!(!rows.is_empty());
    for row in &rows {
        assert!(["El Niño", "La Niña", "Neutral"].contains(&row.phase.as_str()));
        assert!(row.oni.is_finite() || row.oni.is_nan());
    }
}

// ── Thermocline (D20) and Warm Water Volume — built from a synthetic Field4 ──

fn synthetic_pottmp_field() -> Field4 {
    let time = monthly_index(2024, 12);
    let level = GODAS_LEVELS[..8].to_vec();
    let lat: Vec<f64> = (0..5).map(|i| -4.0 + i as f64 * 2.0).collect();
    let lon: Vec<f64> = (0..5).map(|i| 190.0 + i as f64 * 10.0).collect();
    let data = Array4::from_shape_fn(
        (time.len(), level.len(), lat.len(), lon.len()),
        |(t, l, la, lo)| 28.0 - (l as f64) * 1.5 + ((t + la + lo) % 3) as f64 * 0.1,
    );
    Field4 {
        time,
        level,
        lat,
        lon,
        data,
        long_name: "Potential temperature".into(),
        units: "°C".into(),
    }
}

#[test]
fn thermocline_depth_is_bounded_by_max_level() {
    let field = synthetic_pottmp_field();
    let max_level = *field.level.last().unwrap();
    let d20 = thermocline_depth_from_field4(&field, 20.0);
    assert!(d20.data.iter().all(|v| *v <= max_level));
    assert_eq!(
        d20.data.dim(),
        (field.time.len(), field.lat.len(), field.lon.len())
    );
}

#[test]
fn warm_water_volume_is_non_negative() {
    let field = synthetic_pottmp_field();
    let wwv = warm_water_volume_from_field4(&field, 20.0, 1000.0);
    assert_eq!(wwv.len(), field.time.len());
    assert!(wwv.values.iter().all(|v| *v >= 0.0));
}

// ── Field3 basic usage compiles & behaves as documented ─────────────────

#[test]
fn field3_spatial_mean_matches_manual_average() {
    let time = monthly_index(2024, 1);
    let lat = vec![-1.0, 0.0, 1.0];
    let lon = vec![10.0, 20.0];
    let data = Array3::from_shape_vec((1, 3, 2), vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
    let field = Field3 {
        time,
        lat,
        lon,
        data,
        long_name: "test".into(),
        units: "unit".into(),
    };
    let mean = field.spatial_mean();
    assert_eq!(mean.len(), 1);
    assert!((mean[0] - 3.5).abs() < 1e-9); // mean of 1..=6
}
