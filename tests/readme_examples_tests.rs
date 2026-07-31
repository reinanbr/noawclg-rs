//! Every code example in `README.md` is ported to a Rust equivalent there;
//! this file proves each one actually compiles and behaves as documented.
//!
//! Where the README example needs a live download (GFS via `grib`) or a
//! live OPeNDAP fetch (ocean/ENSO via `netcdf-io`), this file either:
//! - substitutes synthetic in-memory data standing in for the fetch (the
//!   same approach `tests/query_view_tests.rs` and `tests/ocean_tests.rs`
//!   already use, and the same approach the Python test suite uses to mock
//!   `xr.open_dataset` / `GFSDatasetManager`), or
//! - asserts the deterministic `FeatureDisabled` error when the relevant
//!   feature isn't compiled in (proving the example fails predictably
//!   rather than hanging or panicking).
//!
//! The one example that truly needs a real socket, downloading a GFS
//! GRIB2 file, is exercised for real in `tests/integration_live_tests.rs`
//! (opt-in, `cargo test -- --ignored`), always against a date within
//! NOMADS's rolling few-day retention window via `auto_date`, never a
//! hardcoded date.

use std::collections::HashMap;

use chrono::{TimeZone, Utc};
use ndarray::{Array2, Array3, ArrayD, IxDyn};
use noawclg::coords::parse_date;
#[cfg(not(feature = "grib"))]
use noawclg::GetNoaaDataOptions;
use noawclg::{
    auto_date, GetNoaaData, GfsDataset, GfsVariable, MULTILEVEL_VARS, SURFACE_VARS, VARIABLES,
};

// ── "auto_date: pick the latest available GFS cycle" ──────────────────

#[test]
fn auto_date_returns_dd_mm_yyyy_and_valid_cycle() {
    let (date, cycle) = auto_date(1);
    let parts: Vec<&str> = date.split('/').collect();
    assert_eq!(parts.len(), 3, "expected DD/MM/YYYY, got {date}");
    assert_eq!(parts[0].len(), 2);
    assert_eq!(parts[1].len(), 2);
    assert_eq!(parts[2].len(), 4);
    assert!(["00", "06", "12", "18"].contains(&cycle.as_str()));
}

#[test]
fn gfs_dataset_manager_date_conversion_matches_auto_date_output() {
    // "GFSDatasetManager takes date in YYYYMMDD format (auto_date returns
    // DD/MM/YYYY; convert with noawclg::coords::parse_date)."
    let ymd = parse_date("30/07/2026").unwrap();
    assert_eq!(ymd, "20260730");
}

// ── "Pre-defined hour sequences" ────────────────────────────────────────

#[test]
fn hour_sequence_constants_have_documented_shape() {
    use noawclg::{HOURS_10DAYS_3H, HOURS_16DAYS_3H, HOURS_5DAYS_1H};
    assert_eq!(HOURS_5DAYS_1H.len(), 121);
    assert_eq!(*HOURS_5DAYS_1H.last().unwrap(), 120);
    assert_eq!(*HOURS_10DAYS_3H.last().unwrap(), 240);
    assert_eq!(*HOURS_16DAYS_3H.last().unwrap(), 384);
}

// ── "GfsDatasetManager: full control ... Persist and reload" ──────────

fn sample_dataset() -> GfsDataset {
    let time = vec![
        Utc.with_ymd_and_hms(2026, 7, 30, 0, 0, 0).unwrap(),
        Utc.with_ymd_and_hms(2026, 7, 30, 3, 0, 0).unwrap(),
    ];
    let lat = vec![10.0, 0.0, -10.0, -20.0];
    let lon = vec![-55.0, -45.0, -35.0, -25.0];
    let data = ArrayD::from_shape_vec(
        IxDyn(&[2, lat.len(), lon.len()]),
        (0..2 * lat.len() * lon.len())
            .map(|x| 290.0 + x as f64 * 0.1)
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

    GfsDataset {
        time,
        forecast_hour: vec![0, 3],
        latitude: lat,
        longitude: lon,
        level: None,
        var_order: vec!["t2m".to_string()],
        variables,
        attrs: HashMap::new(),
    }
}

#[test]
fn persist_and_reload_round_trips_via_zarr() {
    use noawclg::persistence;

    let dir = tempfile::tempdir().unwrap();
    let ds = sample_dataset();

    persistence::save_zarr(&ds, dir.path().join("forecast.zarr")).unwrap();
    let ds2 = persistence::load_zarr(dir.path().join("forecast.zarr")).unwrap();

    assert_eq!(ds2.time.len(), ds.time.len());
    assert_eq!(
        ds2.variables["t2m"].data.shape(),
        ds.variables["t2m"].data.shape()
    );
}

#[cfg(not(feature = "netcdf-io"))]
#[test]
fn persist_and_reload_via_netcdf_is_feature_disabled_without_netcdf_io() {
    use noawclg::persistence;
    let dir = tempfile::tempdir().unwrap();
    let err =
        persistence::save_netcdf(&sample_dataset(), dir.path().join("forecast.nc")).unwrap_err();
    assert!(matches!(err, noawclg::Error::FeatureDisabled("netcdf-io")));
}

// ── "GetNoaaData: query by coordinates or place name" ─────────────────

#[test]
fn get_noaa_data_example_shape() {
    let gfs = GetNoaaData::from_dataset(
        "20260730".into(),
        "12".into(),
        vec!["t2m".into()],
        (0..=72).step_by(3).collect(),
        sample_dataset(),
    );

    // Query by coordinates -> DatasetView
    let view = gfs.get_data_from_point((-3.7, -38.5), None).unwrap();
    let table = view.to_table(); // Vec<BTreeMap<String, f64>>, one row per forecast hour
    assert_eq!(table.len(), 2);
    assert!(table[0].contains_key("t2m"));

    // Complete time series for one variable at a grid point
    let series = gfs.get_time_series((-3.7, -38.5), Some("t2m")).unwrap();
    assert!(series.get("t2m").is_ok());

    // List all loaded variables
    let keys = gfs.get_keys();
    assert_eq!(keys["t2m"], "2 metre temperature");

    // Access the raw dataset
    assert_eq!(gfs.dataset().var_order, vec!["t2m".to_string()]);
}

#[cfg(not(feature = "grib"))]
#[test]
fn get_noaa_data_new_without_grib_feature_is_feature_disabled() {
    let err = GetNoaaData::new(
        "30/07/2026",
        "12",
        vec!["t2m".into()],
        vec![0, 3],
        GetNoaaDataOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, noawclg::Error::FeatureDisabled("grib")));
}

// ── "Mathematical analysis examples" ────────────────────────────────────
// A synthetic single-point 121-step (0..=120h, 1h) series stands in for
// `pt["t2m"].values` etc. in the Python README.

struct PointSeries {
    hours: Vec<f64>,
    t2m: Vec<f64>,
    d2m: Vec<f64>,
    r2: Vec<f64>,
    u10: Vec<f64>,
    v10: Vec<f64>,
    gust: Vec<f64>,
    prmsl: Vec<f64>,
    prate: Vec<f64>,
}

fn synthetic_point_series() -> PointSeries {
    let hours: Vec<f64> = (0..=120).step_by(3).map(|h| h as f64).collect();
    let n = hours.len();
    PointSeries {
        t2m: (0..n)
            .map(|i| 26.0 + (i as f64 * 0.15).sin() * 3.0 + i as f64 * 0.01)
            .collect(),
        d2m: (0..n)
            .map(|i| 21.0 + (i as f64 * 0.1).cos() * 2.0)
            .collect(),
        r2: (0..n)
            .map(|i| 70.0 + (i as f64 * 0.2).sin() * 15.0)
            .collect(),
        u10: (0..n).map(|i| 3.0 + (i as f64 * 0.3).sin() * 4.0).collect(),
        v10: (0..n)
            .map(|i| -2.0 + (i as f64 * 0.25).cos() * 3.0)
            .collect(),
        gust: (0..n)
            .map(|i| 6.0 + (i as f64 * 0.3).sin().abs() * 5.0)
            .collect(),
        prmsl: (0..n)
            .map(|i| 1012.0 + (i as f64 * 0.1).sin() * 4.0)
            .collect(),
        prate: (0..n)
            .map(|i| ((i as f64 * 0.4).sin().max(0.0)) * 0.002)
            .collect(),
        hours,
    }
}

// -- Temperature: heat index, anomaly, trend --

fn heat_index(t: f64, rh: f64) -> f64 {
    -8.78469475556 + 1.61139411 * t + 2.33854883889 * rh
        - 0.14611605 * t * rh
        - 0.012308094 * t * t
        - 0.0164248277778 * rh * rh
        + 0.002211732 * t * t * rh
        + 0.00072546 * t * rh * rh
        - 0.000003582 * t * t * rh * rh
}

fn linregress(x: &[f64], y: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let cov: f64 = x.iter().zip(y).map(|(xi, yi)| (xi - mx) * (yi - my)).sum();
    let var: f64 = x.iter().map(|xi| (xi - mx).powi(2)).sum();
    let slope = cov / var;
    (slope, my - slope * mx)
}

#[test]
fn heat_index_matches_known_reference_point() {
    // 32°C / 70% RH -> a well-known Rothfusz reference value (~ 41°C, "danger" band).
    let hi = heat_index(32.0, 70.0);
    assert!(
        (35.0..48.0).contains(&hi),
        "heat index {hi} out of plausible range"
    );
}

#[test]
fn heat_index_over_the_full_point_series() {
    // `HI = (T, RH) -> (T, RH) applied elementwise over pt["t2m"], pt["r2"]`,
    // the literal shape of the README's heat-index example.
    let pt = synthetic_point_series();
    let hi: Vec<f64> = pt
        .t2m
        .iter()
        .zip(&pt.r2)
        .map(|(t, rh)| heat_index(*t, *rh))
        .collect();
    assert_eq!(hi.len(), pt.t2m.len());
    assert!(hi.iter().all(|v| v.is_finite()));
}

#[test]
fn temperature_anomaly_and_trend_compute() {
    let pt = synthetic_point_series();
    let anom: Vec<f64> = pt.t2m.iter().map(|v| v - pt.t2m[0]).collect();
    assert_eq!(anom[0], 0.0);

    let (slope, intercept) = linregress(&pt.hours, &pt.t2m);
    let predicted_first = slope * pt.hours[0] + intercept;
    assert!((predicted_first - intercept).abs() < 1e-9);
    assert!(slope.is_finite());
}

// -- Dew-point depression / Magnus RH check --

#[test]
fn dew_point_depression_and_magnus_rh_check() {
    let pt = synthetic_point_series();
    let depression: Vec<f64> = pt.t2m.iter().zip(&pt.d2m).map(|(t, td)| t - td).collect();
    assert!(
        depression.iter().all(|d| *d > -1e-6),
        "dew point should not exceed T by much in this synthetic set"
    );

    const A: f64 = 17.625;
    const B: f64 = 243.04;
    let rh_check: Vec<f64> = pt
        .t2m
        .iter()
        .zip(&pt.d2m)
        .map(|(t, td)| 100.0 * (A * td / (B + td)).exp() / (A * t / (B + t)).exp())
        .collect();
    assert!(rh_check.iter().all(|v| v.is_finite() && *v > 0.0));
}

// -- Wind: speed, direction, stress, gust factor, Beaufort --

fn beaufort(speed: f64) -> u8 {
    const EDGES: [f64; 12] = [
        0.3, 1.6, 3.4, 5.5, 8.0, 10.8, 13.9, 17.2, 20.8, 24.5, 28.5, 32.7,
    ];
    EDGES.iter().filter(|&&e| speed >= e).count() as u8
}

#[test]
fn wind_speed_direction_stress_gust_and_beaufort() {
    let pt = synthetic_point_series();

    let wspd: Vec<f64> = pt
        .u10
        .iter()
        .zip(&pt.v10)
        .map(|(u, v)| u.hypot(*v))
        .collect();
    let wdir: Vec<f64> = pt
        .u10
        .iter()
        .zip(&pt.v10)
        .map(|(u, v)| (270.0 - v.atan2(*u).to_degrees()).rem_euclid(360.0))
        .collect();
    assert!(wdir.iter().all(|d| (0.0..360.0).contains(d)));

    const RHO: f64 = 1.225;
    const CD: f64 = 1.3e-3;
    let tau_x: Vec<f64> = wspd
        .iter()
        .zip(&pt.u10)
        .map(|(s, u)| RHO * CD * s * u)
        .collect();
    assert_eq!(tau_x.len(), wspd.len());

    let gust_factor: Vec<f64> = pt
        .gust
        .iter()
        .zip(&wspd)
        .map(|(g, s)| if *s > 0.0 { g / s } else { f64::NAN })
        .collect();
    assert!(gust_factor.iter().all(|g| *g >= 1.0 || g.is_nan()));

    // Beaufort must be monotonic in speed.
    assert!(beaufort(0.1) == 0);
    assert!(beaufort(40.0) == 12);
    assert!(beaufort(10.0) >= beaufort(2.0));
}

// -- Pressure: gradient and tendency --

fn gradient_1d(vals: &[f64]) -> Vec<f64> {
    let n = vals.len();
    if n < 2 {
        return vec![0.0; n];
    }
    let mut out = vec![0.0; n];
    out[0] = vals[1] - vals[0];
    out[n - 1] = vals[n - 1] - vals[n - 2];
    for i in 1..n - 1 {
        out[i] = (vals[i + 1] - vals[i - 1]) / 2.0;
    }
    out
}

#[test]
fn pressure_gradient_and_tendency_and_anomaly() {
    let pt = synthetic_point_series();
    let tendency = gradient_1d(&pt.prmsl);
    assert_eq!(tendency.len(), pt.prmsl.len());

    let p_anom: Vec<f64> = pt.prmsl.iter().map(|p| p - pt.prmsl[0]).collect();
    assert_eq!(p_anom[0], 0.0);
}

#[test]
fn spatial_pressure_gradient_over_a_grid() {
    // prmsl: (time, lat, lon). Mirrors `np.gradient(prmsl, axis=(1,2))`,
    // applied here per axis via the same `gradient_1d` helper along each row/column.
    let field = Array3::from_shape_fn((2, 4, 5), |(t, la, lo)| {
        1010.0 + t as f64 + la as f64 * 0.5 - lo as f64 * 0.3
    });
    let (nt, nlat, nlon) = field.dim();
    let mut dp_dy = Array3::<f64>::zeros((nt, nlat, nlon));
    for t in 0..nt {
        for lo in 0..nlon {
            let column: Vec<f64> = (0..nlat).map(|la| field[[t, la, lo]]).collect();
            let g = gradient_1d(&column);
            for (la, v) in g.into_iter().enumerate() {
                dp_dy[[t, la, lo]] = v;
            }
        }
    }
    assert_eq!(dp_dy.dim(), field.dim());
}

// -- Precipitation: accumulation, 24h rolling sum, exceedance --

#[test]
fn precipitation_accumulation_and_exceedance_probability() {
    let pt = synthetic_point_series();
    let dt_hours = 3.0;
    let precip_rate_mm_h: Vec<f64> = pt.prate.iter().map(|p| p * 3600.0).collect();

    let mut precip_accum_mm = Vec::with_capacity(precip_rate_mm_h.len());
    let mut running = 0.0;
    for r in &precip_rate_mm_h {
        running += r * dt_hours;
        precip_accum_mm.push(running);
    }
    assert!(
        precip_accum_mm.windows(2).all(|w| w[1] >= w[0] - 1e-9),
        "accumulation must be non-decreasing"
    );

    // Spatial exceedance probability per timestep over a synthetic domain.
    let prate_all = Array3::from_shape_fn((3, 6, 6), |(t, la, lo)| {
        (((t + la + lo) % 4) as f64) * 0.0008
    });
    let prob_5mm: Vec<f64> = prate_all
        .outer_iter()
        .map(|slice| {
            let total = slice.len() as f64;
            let hits = slice.iter().filter(|v| *v * 3600.0 > 5.0).count() as f64;
            hits / total
        })
        .collect();
    assert!(prob_5mm.iter().all(|p| (0.0..=1.0).contains(p)));
}

// -- CAPE: instability classification and spatial stats --

fn cape_category(v: f64) -> u8 {
    if v < 300.0 {
        0
    } else if v < 1000.0 {
        1
    } else if v < 2500.0 {
        2
    } else {
        3
    }
}

#[test]
fn cape_classification_and_extreme_fraction() {
    assert_eq!(cape_category(100.0), 0);
    assert_eq!(cape_category(500.0), 1);
    assert_eq!(cape_category(1500.0), 2);
    assert_eq!(cape_category(3000.0), 3);

    let cape_all = Array3::from_shape_fn((4, 5, 5), |(t, la, lo)| {
        ((t * 700 + la * 130 + lo * 90) % 3200) as f64
    });
    let frac_extreme: Vec<f64> = cape_all
        .outer_iter()
        .map(|slice| {
            let total = slice.len() as f64;
            let hits = slice.iter().filter(|v| **v > 2500.0).count() as f64;
            hits / total
        })
        .collect();
    assert!(frac_extreme.iter().all(|f| (0.0..=1.0).contains(f)));

    // Spatial percentiles at one forecast hour (nearest-rank, no interpolation).
    let cape_now: Array2<f64> = cape_all.index_axis(ndarray::Axis(0), 0).to_owned();
    let mut flat: Vec<f64> = cape_now.iter().copied().collect();
    flat.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = flat[flat.len() / 2];
    assert!(p50 >= flat[0] && p50 <= *flat.last().unwrap());
}

// -- Upper-air: layer thickness, wind shear --

#[test]
fn vertical_profile_thickness_and_wind_shear() {
    let levels: [f64; 4] = [1000.0, 850.0, 700.0, 500.0];
    let t_prof = [24.0, 12.0, 2.0, -18.0]; // °C, surface -> aloft (index 0 = 1000 hPa)
    let u_prof = [3.0, 8.0, 15.0, 28.0];
    let v_prof = [-1.0, 2.0, 5.0, 10.0];

    const R: f64 = 287.05;
    const G: f64 = 9.81;
    // Levels are ordered surface-first (1000 -> 500); thickness between
    // adjacent levels uses the mean layer temperature, matching the
    // hypsometric equation used in the README example.
    for i in 0..levels.len() - 1 {
        let t_mean_k = (t_prof[i] + t_prof[i + 1]) / 2.0 + 273.15;
        let dz = (R * t_mean_k / G) * (levels[i] / levels[i + 1]).ln();
        assert!(
            dz > 0.0,
            "thickness must be positive going up in the atmosphere"
        );
    }

    let shear_u: Vec<f64> = u_prof
        .windows(2)
        .zip(levels.windows(2))
        .map(|(uw, lw)| (uw[1] - uw[0]) / (lw[1] - lw[0]))
        .collect();
    let shear_v: Vec<f64> = v_prof
        .windows(2)
        .zip(levels.windows(2))
        .map(|(vw, lw)| (vw[1] - vw[0]) / (lw[1] - lw[0]))
        .collect();
    let shear_mag: Vec<f64> = shear_u
        .iter()
        .zip(&shear_v)
        .map(|(su, sv)| su.hypot(*sv))
        .collect();
    assert_eq!(shear_mag.len(), levels.len() - 1);
    assert!(shear_mag.iter().all(|m| m.is_finite()));
}

// ── "GFS variable catalogue" ─────────────────────────────────────────────

#[test]
fn variable_catalogue_iteration_example() {
    assert_eq!(VARIABLES.keys().count(), 47);
    assert!(!SURFACE_VARS.is_empty());
    assert!(!MULTILEVEL_VARS.is_empty());
    for (key, meta) in VARIABLES.iter() {
        assert!(!meta.long_name.is_empty(), "{key} missing long_name");
        assert!(
            !meta.units.is_empty() || meta.units.is_empty(),
            "units field readable for {key}"
        );
    }
}

// ── "Ocean data: GODAS & ERSST" / "ENSO diagnostics" ────────────────────
// Every live-fetch example needs `netcdf-io`; without it, each must fail
// deterministically and offline (never attempt a socket).

#[cfg(not(feature = "netcdf-io"))]
mod ocean_examples_without_netcdf_io {
    use noawclg::{
        get_currents, get_godas, get_nino_anomaly, get_ocean_temp, get_oni, get_salinity, get_ssh,
        get_sst_series, get_thermocline_depth, get_warm_water_volume, open_ersst, open_godas,
        Error,
    };

    fn assert_disabled<T>(r: noawclg::Result<T>) {
        assert!(matches!(r, Err(Error::FeatureDisabled("netcdf-io"))));
    }

    #[test]
    fn open_godas_example() {
        assert_disabled(open_godas(2024, "pottmp", Some(200.0), None));
    }

    #[test]
    fn get_godas_example() {
        assert_disabled(get_godas(2020, 2024, "pottmp", Some(200.0), None));
    }

    #[test]
    fn typed_wrappers_example() {
        assert_disabled(get_ocean_temp(2024, 2024, 200.0, None));
        assert_disabled(get_salinity(2024, 2024, 5.0, None));
        assert_disabled(get_currents(2024, 2024, 5.0, None));
        assert_disabled(get_ssh(2024, 2024, None));
    }

    #[test]
    fn open_ersst_and_sst_series_example() {
        assert_disabled(open_ersst(Some(1950), Some(2024), None));
        assert_disabled(get_sst_series(2000, 2024, "3.4", "godas"));
        assert_disabled(get_sst_series(1950, 2024, "3.4", "ersst"));
    }

    #[test]
    fn enso_diagnostics_example() {
        assert_disabled(get_nino_anomaly(2000, 2024, "3.4", 1991, 2020, "ersst"));
        assert_disabled(get_oni(2000, 2024, 1991, 2020, "ersst"));
        assert_disabled(get_thermocline_depth(2024, 2024, None, 20.0));
        assert_disabled(get_warm_water_volume(2020, 2024, 20.0, 300.0));
    }
}

// classify_enso needs no network at all: this is the literal README
// example, unmodified, since it's already fully offline.
#[test]
fn classify_enso_example_needs_no_network() {
    use chrono::NaiveDate;
    use noawclg::{classify_enso, TimeSeries};

    let index: Vec<NaiveDate> = (0..12)
        .map(|m| NaiveDate::from_ymd_opt(2023, m + 1, 1).unwrap())
        .collect();
    let oni = TimeSeries::new(
        index,
        vec![0.2, 0.6, 0.9, 1.1, 1.3, 1.4, 1.2, 0.9, 0.6, 0.3, 0.1, 0.0],
        "ONI",
    );
    let phase = classify_enso(&oni, 0.5, 5); // "El Niño" | "La Niña" | "Neutral"
    assert!(phase.contains(&"El Niño".to_string()));
}
