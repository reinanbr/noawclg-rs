//! High-level query API: geocoding, point selection, and time-series access.
//!
//! Direct port of `noawclg/query.py`.

use std::collections::HashMap;
use std::time::Duration;

use ndarray::{ArrayD, Axis};
use serde::Deserialize;

use crate::catalog::VARIABLES;
use crate::coords::{normalize_lon, parse_date, BoundingBox, Region};
use crate::error::{Error, Result};
use crate::gfs_dataset::{GfsDataset, GfsDatasetManager};
use crate::view::{DatasetView, SelectedVariable};

/// `(lat, lon)` in degrees.
pub type Coordinate = (f64, f64);

/// Index of the nearest value to `target`. On an exact tie, prefers the
/// later (higher-index) candidate, matching `xarray`/`pandas`'s observed
/// `method="nearest"` tie-break (see `tests/test_main.py::
/// test_get_data_from_point_selects_nearest`, where lon 321.5 sits exactly
/// between 321 and 322 and xarray resolves it to 322).
fn nearest_index(vals: &[f64], target: f64) -> usize {
    let mut best = 0usize;
    let mut best_dist = f64::INFINITY;
    for (i, v) in vals.iter().enumerate() {
        let dist = (v - target).abs();
        if dist <= best_dist {
            best = i;
            best_dist = dist;
        }
    }
    best
}

fn select_point(
    v: &crate::gfs_dataset::GfsVariable,
    lat_idx: usize,
    lon_idx: usize,
) -> SelectedVariable {
    let ndim = v.data.ndim();
    let step1 = v.data.index_axis(Axis(ndim - 1), lon_idx);
    let step2 = step1.index_axis(Axis(ndim - 2), lat_idx);
    let data: ArrayD<f64> = step2.to_owned().into_dyn();
    let dims: Vec<String> = v.dims[..v.dims.len() - 2].to_vec();
    SelectedVariable {
        data,
        dims,
        long_name: v.long_name.clone(),
        units: v.units.clone(),
    }
}

/// Opens a GFS dataset via NOMADS and exposes spatial/temporal queries.
///
/// Mirrors `noawclg.query.get_noaa_data`.
#[derive(Debug)]
pub struct GetNoaaData {
    pub date: String, // YYYYMMDD
    pub cycle: String,
    pub keys: Vec<String>,
    pub hours: Vec<u32>,
    ds: GfsDataset,
}

pub struct GetNoaaDataOptions {
    pub region: Option<Region>,
    pub output_dir: String,
}

impl Default for GetNoaaDataOptions {
    fn default() -> Self {
        GetNoaaDataOptions {
            region: None,
            output_dir: "./gfs_output".to_string(),
        }
    }
}

impl GetNoaaData {
    /// `date` in `'DD/MM/YYYY'` format (same convention as `auto_date`).
    /// Building the dataset requires the `grib` feature; without it this
    /// returns [`crate::Error::FeatureDisabled`].
    pub fn new(
        date: &str,
        cycle: &str,
        keys: Vec<String>,
        hours: Vec<u32>,
        options: GetNoaaDataOptions,
    ) -> Result<Self> {
        let date_ymd = parse_date(date)?;

        let unknown: Vec<String> = keys
            .iter()
            .filter(|k| !VARIABLES.contains_key(k.as_str()))
            .cloned()
            .collect();
        if !unknown.is_empty() {
            return Err(Error::UnknownVariables(unknown));
        }

        let manager =
            GfsDatasetManager::new(&date_ymd, cycle, &options.output_dir, options.region)?;
        let key_refs: Vec<&str> = keys.iter().map(|s| s.as_str()).collect();
        let ds = if keys.len() > 1 {
            manager.build_multi_dataset(&key_refs, &hours)?
        } else {
            manager.build_dataset(key_refs[0], &hours)?
        };

        Ok(GetNoaaData {
            date: date_ymd,
            cycle: cycle.to_string(),
            keys,
            hours,
            ds,
        })
    }

    /// Wrap an already-built [`GfsDataset`] directly, bypassing
    /// [`GfsDatasetManager`] entirely — useful for datasets loaded from
    /// disk via [`crate::persistence`], or for tests that want to exercise
    /// [`Self::get_data_from_point`] / [`Self::get_time_series`] without a
    /// network-backed build step.
    pub fn from_dataset(
        date: String,
        cycle: String,
        keys: Vec<String>,
        hours: Vec<u32>,
        ds: GfsDataset,
    ) -> Self {
        GetNoaaData {
            date,
            cycle,
            keys,
            hours,
            ds,
        }
    }

    pub fn get_keys(&self) -> HashMap<String, String> {
        self.ds.keys()
    }

    pub fn bounds(&self) -> BoundingBox {
        BoundingBox::new(
            self.ds
                .latitude
                .iter()
                .cloned()
                .fold(f64::INFINITY, f64::min),
            self.ds
                .latitude
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max),
            self.ds
                .longitude
                .iter()
                .cloned()
                .fold(f64::INFINITY, f64::min),
            self.ds
                .longitude
                .iter()
                .cloned()
                .fold(f64::NEG_INFINITY, f64::max),
        )
    }

    /// Underlying dataset, for advanced use.
    pub fn dataset(&self) -> &GfsDataset {
        &self.ds
    }

    /// Consume `self` and take ownership of the underlying dataset.
    pub fn into_dataset(self) -> GfsDataset {
        self.ds
    }

    /// Select the nearest grid point to `(lat, lon)`.
    ///
    /// Mirrors `get_noaa_data.get_data_from_point`.
    pub fn get_data_from_point(
        &self,
        point: Coordinate,
        tolerance: Option<f64>,
    ) -> Result<DatasetView> {
        let (lat, lon) = point;
        let lon_min = self
            .ds
            .longitude
            .iter()
            .cloned()
            .fold(f64::INFINITY, f64::min);
        let lon = normalize_lon(lon, lon_min);

        let bounds = self.bounds();
        if !bounds.contains(lat, lon) {
            eprintln!(
                "[noawclg] point (lat={lat:.4}, lon={lon:.4}) is outside dataset bounds [{bounds}]. Nearest edge point returned."
            );
        }

        let lat_idx = nearest_index(&self.ds.latitude, lat);
        let lon_idx = nearest_index(&self.ds.longitude, lon);

        if let Some(tol) = tolerance {
            let dlat = (self.ds.latitude[lat_idx] - lat).abs();
            let dlon = (self.ds.longitude[lon_idx] - lon).abs();
            if dlat > tol || dlon > tol {
                return Err(Error::other(format!(
                    "nearest grid point (lat={:.4}, lon={:.4}) exceeds tolerance {tol}",
                    self.ds.latitude[lat_idx], self.ds.longitude[lon_idx]
                )));
            }
        }

        let mut variables = HashMap::new();
        for name in &self.ds.var_order {
            if let Some(v) = self.ds.variables.get(name) {
                variables.insert(name.clone(), select_point(v, lat_idx, lon_idx));
            }
        }

        Ok(DatasetView {
            time: self.ds.time.clone(),
            forecast_hour: self.ds.forecast_hour.clone(),
            latitude: self.ds.latitude[lat_idx],
            longitude: self.ds.longitude[lon_idx],
            level: self.ds.level.clone(),
            var_order: self.ds.var_order.clone(),
            variables,
        })
    }

    /// Geocode `place` (via OpenStreetMap Nominatim) and return data from
    /// the nearest grid point.
    ///
    /// Mirrors `get_noaa_data.get_data_from_place`.
    pub fn get_data_from_place(&self, place: &str, tolerance: Option<f64>) -> Result<DatasetView> {
        let (lat, lon) = geocode(place)?;
        self.get_data_from_point((lat, lon), tolerance)
    }

    /// Complete time series at the nearest grid point, optionally isolated
    /// to a single variable. Mirrors `get_noaa_data.get_time_series`.
    pub fn get_time_series(
        &self,
        point: Coordinate,
        variable: Option<&str>,
    ) -> Result<DatasetView> {
        let view = self.get_data_from_point(point, None)?;
        if let Some(var) = variable {
            view.get(var)?;
        }
        Ok(view)
    }
}

#[derive(Deserialize)]
struct NominatimResult {
    lat: String,
    lon: String,
}

/// Geocode a place name via the public OpenStreetMap Nominatim API (the
/// same service `geopy.geocoders.Nominatim` calls in Python).
pub fn geocode(place: &str) -> Result<(f64, f64)> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("noawclg-rs/2.3.0")
        .timeout(Duration::from_secs(10))
        .build()?;

    let results: Vec<NominatimResult> = client
        .get("https://nominatim.openstreetmap.org/search")
        .query(&[("q", place), ("format", "json"), ("limit", "1")])
        .send()?
        .json()?;

    let first = results
        .into_iter()
        .next()
        .ok_or_else(|| Error::GeocodeNotFound(place.to_string()))?;

    let lat: f64 = first
        .lat
        .parse()
        .map_err(|_| Error::GeocodeNotFound(place.to_string()))?;
    let lon: f64 = first
        .lon
        .parse()
        .map_err(|_| Error::GeocodeNotFound(place.to_string()))?;
    Ok((lat, lon))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfs_dataset::GfsVariable;
    use ndarray::IxDyn;

    fn sample_dataset() -> GfsDataset {
        let mut ds = GfsDataset {
            time: vec![chrono::Utc::now()],
            forecast_hour: vec![0, 3],
            latitude: vec![-4.0, -3.0, -2.0],
            longitude: vec![320.0, 321.0, 322.0],
            ..Default::default()
        };
        let data =
            ArrayD::from_shape_vec(IxDyn(&[2, 3, 3]), (0..18).map(|x| x as f64).collect()).unwrap();
        ds.var_order.push("t2m".to_string());
        ds.variables.insert(
            "t2m".to_string(),
            GfsVariable {
                data,
                dims: vec!["time".into(), "latitude".into(), "longitude".into()],
                long_name: "2 metre temperature".into(),
                units: "C".into(),
            },
        );
        ds
    }

    #[test]
    fn select_point_picks_nearest_lon() {
        let ds = sample_dataset();
        let noaa = GetNoaaData {
            date: "20260403".into(),
            cycle: "00".into(),
            keys: vec!["t2m".into()],
            hours: vec![0, 3],
            ds,
        };
        let view = noaa.get_data_from_point((-3.1, -38.5), None).unwrap();
        assert!((view.longitude - 322.0).abs() < 1e-9);
    }

    #[test]
    fn get_time_series_missing_variable_errors() {
        let ds = sample_dataset();
        let noaa = GetNoaaData {
            date: "20260403".into(),
            cycle: "00".into(),
            keys: vec!["t2m".into()],
            hours: vec![0, 3],
            ds,
        };
        let err = noaa
            .get_time_series((-3.1, -38.5), Some("prate"))
            .unwrap_err();
        assert!(err.to_string().contains("Variable 'prate' not found"));
    }
}
