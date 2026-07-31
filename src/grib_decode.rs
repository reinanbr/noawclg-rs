//! GRIB2 → [`GfsDataset`] decoding, backed by the pure-Rust [`gribberish`]
//! crate. Only compiled with `--features grib`.
//!
//! This is the Rust analogue of the extraction half of
//! `noawclg/gfs_dataset.py` (`_open_var` / `_extract` / `_build_single_var_ds`),
//! which in Python is done by `cfgrib` (a `libeccodes` wrapper). Because
//! `gribberish` and `cfgrib` are different decoders, this module re-derives
//! the same result (one time-stacked array per variable, with correctly
//! oriented lat/lon axes) directly from `gribberish`'s message API rather
//! than trying to mimic `cfgrib`'s internals.

use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use gribberish::message::read_messages;
use ndarray::{ArrayD, Axis, IxDyn};

use crate::catalog::{VarConfig, VARIABLES};
use crate::error::{Error, Result};
use crate::gfs_dataset::{GfsDataset, GfsVariable};

/// Match a decoded level value (whatever unit `gribberish` reports it in)
/// back to one of the requested `cfg.levels` (hPa). NOAA's grib-filter only
/// ever returns the levels we asked for, so a nearest-match against the raw
/// value, the value/100 (Pa→hPa) is enough to disambiguate without needing
/// to trust a specific unit convention.
fn match_level(levels: &[i32], raw: Option<f64>, fallback_index: usize) -> f64 {
    if let Some(v) = raw {
        let candidates = [v, v / 100.0];
        for c in candidates {
            if let Some(best) = levels.iter().min_by(|a, b| {
                (**a as f64 - c)
                    .abs()
                    .partial_cmp(&(**b as f64 - c).abs())
                    .unwrap()
            }) {
                if (*best as f64 - c).abs() < 1.0 {
                    return *best as f64;
                }
            }
        }
    }
    levels.get(fallback_index).copied().unwrap_or(0) as f64
}

fn apply_converter(cfg: &VarConfig, x: f64) -> f64 {
    match cfg.converter {
        Some(f) => f(x),
        None => x,
    }
}

/// Decode one variable from every cached GRIB2 file and stack along time.
///
/// Mirrors `GFSDatasetManager._build_single_var_ds`.
pub fn build_single_var_dataset(
    var_key: &str,
    files: &BTreeMap<u32, PathBuf>,
    run_dt: NaiveDateTime,
    date: &str,
    cycle: &str,
) -> Result<GfsDataset> {
    let cfg = *VARIABLES
        .get(var_key)
        .ok_or_else(|| Error::UnknownVariables(vec![var_key.to_string()]))?;
    let abbrev = cfg.grib_var.trim_start_matches("var_");

    let mut times: Vec<NaiveDateTime> = Vec::new();
    let mut fhours: Vec<u32> = Vec::new();
    let mut slices: Vec<ArrayD<f64>> = Vec::new();
    let mut lat_ref: Option<Vec<f64>> = None;
    let mut lon_ref: Option<Vec<f64>> = None;
    let mut level_ref: Option<Vec<f64>> = None;

    for (&hour, path) in files.iter() {
        let bytes = std::fs::read(path)?;
        let matches: Vec<_> = read_messages(&bytes)
            .filter(|m| {
                m.variable_abbrev()
                    .map(|a| a.eq_ignore_ascii_case(abbrev))
                    .unwrap_or(false)
            })
            .collect();
        if matches.is_empty() {
            continue;
        }

        let is_ml = cfg.multilevel && cfg.levels.is_some();

        if is_ml {
            let levels = cfg.levels.unwrap();
            let mut layer_data: Vec<(f64, Vec<f64>)> = Vec::with_capacity(matches.len());
            let (mut h, mut w) = (0usize, 0usize);

            for (idx, msg) in matches.iter().enumerate() {
                let Ok((height, width)) = msg.grid_dimensions() else {
                    continue;
                };
                let Ok(data) = msg.data() else { continue };
                h = height;
                w = width;

                if lat_ref.is_none() {
                    if let Ok(proj) = msg.latlng_projector() {
                        let (lats, lons) = proj.lat_lng_adjusted(true, true);
                        let data_adj = proj.adjust_data(data.clone(), true, true);
                        lat_ref = Some(lats);
                        lon_ref = Some(lons);
                        let raw = msg.first_fixed_surface().ok().and_then(|(_, v)| v);
                        layer_data.push((match_level(levels, raw, idx), data_adj));
                        continue;
                    }
                }
                let raw = msg.first_fixed_surface().ok().and_then(|(_, v)| v);
                layer_data.push((match_level(levels, raw, idx), data));
            }
            if layer_data.is_empty() {
                continue;
            }
            layer_data.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
            if level_ref.is_none() {
                level_ref = Some(layer_data.iter().map(|(l, _)| *l).collect());
            }

            let nlev = layer_data.len();
            let mut arr = ArrayD::<f64>::zeros(IxDyn(&[nlev, h, w]));
            for (li, (_, layer)) in layer_data.iter().enumerate() {
                for r in 0..h {
                    for c in 0..w {
                        arr[[li, r, c]] = apply_converter(&cfg, layer[r * w + c]);
                    }
                }
            }
            slices.push(arr);
        } else {
            let msg = &matches[0];
            let Ok((h, w)) = msg.grid_dimensions() else {
                continue;
            };
            let Ok(data) = msg.data() else { continue };

            let data = if let Ok(proj) = msg.latlng_projector() {
                if lat_ref.is_none() {
                    let (lats, lons) = proj.lat_lng_adjusted(true, true);
                    lat_ref = Some(lats);
                    lon_ref = Some(lons);
                }
                proj.adjust_data(data, true, true)
            } else {
                data
            };

            let mut arr = ArrayD::<f64>::zeros(IxDyn(&[h, w]));
            for r in 0..h {
                for c in 0..w {
                    arr[[r, c]] = apply_converter(&cfg, data[r * w + c]);
                }
            }
            slices.push(arr);
        }

        times.push(run_dt + chrono::Duration::hours(hour as i64));
        fhours.push(hour);
    }

    if slices.is_empty() {
        return Err(Error::NoDataForVariable(var_key.to_string()));
    }

    let dims: Vec<String> = if level_ref.is_some() {
        vec![
            "time".into(),
            "level".into(),
            "latitude".into(),
            "longitude".into(),
        ]
    } else {
        vec!["time".into(), "latitude".into(), "longitude".into()]
    };

    let mut shape = vec![slices.len()];
    shape.extend(slices[0].shape());
    let mut full = ArrayD::<f64>::zeros(IxDyn(&shape));
    for (i, sl) in slices.iter().enumerate() {
        full.index_axis_mut(Axis(0), i).assign(sl);
    }

    let times_utc: Vec<DateTime<Utc>> = times.iter().map(|t| Utc.from_utc_datetime(t)).collect();

    let mut ds = GfsDataset {
        time: times_utc,
        forecast_hour: fhours,
        latitude: lat_ref.unwrap_or_default(),
        longitude: lon_ref.unwrap_or_default(),
        level: level_ref,
        var_order: vec![var_key.to_string()],
        ..Default::default()
    };
    ds.variables.insert(
        var_key.to_string(),
        GfsVariable {
            data: full,
            dims,
            long_name: cfg.long_name.to_string(),
            units: cfg.units.to_string(),
        },
    );
    ds.attrs.insert("run_date".into(), date.to_string());
    ds.attrs.insert("run_cycle".into(), cycle.to_string());
    Ok(ds)
}
