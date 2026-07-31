//! Ocean data access: GODAS subsurface fields and ENSO diagnostics.
//!
//! Direct port of `noawclg/ocean.py`. The library is split in two layers:
//!
//! * **Pure / always compiled**: masking, unit conversion, region and depth
//!   selection, and every ENSO index computation (anomaly, ONI, phase
//!   classification, D20 thermocline, WWV, summary table). These operate on
//!   plain [`Field3`]/[`Field4`]/[`TimeSeries`] values and have no network
//!   dependency, mirroring how `tests/test_ocean.py` exercises the Python
//!   library entirely on synthetic in-memory data.
//! * **Live fetch, behind the `netcdf-io` feature**: `open_godas`,
//!   `open_ersst` and friends, which pull GODAS/ERSST over OPeNDAP via the
//!   system `libnetcdf` (the same runtime dependency `xr.open_dataset(url,
//!   engine="netcdf4")` has in Python).

use std::collections::HashMap;
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDate};
use ndarray::{Array3, Array4, Axis};

use crate::coords::BoundingBox;
use crate::error::{Error, Result};

// ══════════════════════════════════════════════════════════════════════════
// Catalogues
// ══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy)]
pub struct GodasVarMeta {
    pub long_name: &'static str,
    pub units_in: &'static str,
    pub units_out: &'static str,
    pub has_levels: bool,
    pub valid_min: Option<f64>,
}

pub static GODAS_VARS: LazyLock<HashMap<&'static str, GodasVarMeta>> = LazyLock::new(|| {
    HashMap::from([
        (
            "pottmp",
            GodasVarMeta {
                long_name: "Potential temperature",
                units_in: "K",
                units_out: "°C",
                has_levels: true,
                valid_min: Some(200.0),
            },
        ),
        (
            "salt",
            GodasVarMeta {
                long_name: "Salinity",
                units_in: "kg/kg",
                units_out: "PSU",
                has_levels: true,
                valid_min: Some(0.001),
            },
        ),
        (
            "ucur",
            GodasVarMeta {
                long_name: "U-component of ocean current (eastward)",
                units_in: "m/s",
                units_out: "m/s",
                has_levels: true,
                valid_min: None,
            },
        ),
        (
            "vcur",
            GodasVarMeta {
                long_name: "V-component of ocean current (northward)",
                units_in: "m/s",
                units_out: "m/s",
                has_levels: true,
                valid_min: None,
            },
        ),
        (
            "sshg",
            GodasVarMeta {
                long_name: "Sea Surface Height Relative to Geoid",
                units_in: "m",
                units_out: "m",
                has_levels: false,
                valid_min: None,
            },
        ),
    ])
});

#[derive(Debug, Clone, Copy)]
pub struct NinoBox {
    pub lat: (f64, f64),
    pub lon: (f64, f64),
}

/// Standard ENSO monitoring regions (longitude in 0–360 convention).
pub static NINO_BOXES: LazyLock<HashMap<&'static str, NinoBox>> = LazyLock::new(|| {
    HashMap::from([
        (
            "1+2",
            NinoBox {
                lat: (-10.0, 0.0),
                lon: (270.0, 280.0),
            },
        ),
        (
            "3",
            NinoBox {
                lat: (-5.0, 5.0),
                lon: (210.0, 270.0),
            },
        ),
        (
            "3.4",
            NinoBox {
                lat: (-5.0, 5.0),
                lon: (190.0, 240.0),
            },
        ),
        (
            "4",
            NinoBox {
                lat: (-5.0, 5.0),
                lon: (160.0, 210.0),
            },
        ),
    ])
});

/// Warm water volume box: 5°S–5°N, 120°E–80°W.
#[allow(dead_code)] // only read by `wwv_box()`, used under the `netcdf-io` feature
const WWV_BOX: NinoBox = NinoBox {
    lat: (-5.0, 5.0),
    lon: (120.0, 280.0),
};

/// GODAS's 40 fixed depth levels (m).
pub static GODAS_LEVELS: LazyLock<Vec<f64>> = LazyLock::new(|| {
    vec![
        5.0, 15.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0, 85.0, 95.0, 105.0, 115.0, 125.0, 135.0,
        145.0, 155.0, 165.0, 175.0, 185.0, 195.0, 205.0, 215.0, 225.0, 238.0, 262.0, 303.0, 366.0,
        459.0, 584.0, 747.0, 949.0, 1193.0, 1479.0, 1807.0, 2174.0, 2579.0, 3016.0, 3483.0, 3972.0,
        4478.0,
    ]
});

const K_TO_C: f64 = 273.15;
const KGK_TO_PSU: f64 = 1000.0;

// ══════════════════════════════════════════════════════════════════════════
// Core field types
// ══════════════════════════════════════════════════════════════════════════

/// A `(time, lat, lon)` gridded field: GODAS surface variables (`sshg`),
/// depth-selected GODAS variables, or ERSST SST.
#[derive(Debug, Clone)]
pub struct Field3 {
    pub time: Vec<NaiveDate>,
    pub lat: Vec<f64>,
    pub lon: Vec<f64>,
    pub data: Array3<f64>,
    pub long_name: String,
    pub units: String,
}

/// A `(time, level, lat, lon)` gridded field: GODAS variables with depth
/// levels, before depth selection.
#[derive(Debug, Clone)]
pub struct Field4 {
    pub time: Vec<NaiveDate>,
    pub level: Vec<f64>,
    pub lat: Vec<f64>,
    pub lon: Vec<f64>,
    pub data: Array4<f64>,
    pub long_name: String,
    pub units: String,
}

/// A GODAS field, which may or may not carry a `level` dimension depending
/// on whether a depth was selected (mirrors the Python code's dynamic
/// `xr.Dataset` shape).
#[derive(Debug, Clone)]
pub enum OceanField {
    Leveled(Field4),
    Surface(Field3),
}

impl OceanField {
    pub fn into_surface(self) -> Result<Field3> {
        match self {
            OceanField::Surface(f) => Ok(f),
            OceanField::Leveled(_) => {
                Err(Error::other("expected a depth-selected (time, lat, lon) field, got (time, level, lat, lon); pass depth_m"))
            }
        }
    }

    pub fn time(&self) -> &[NaiveDate] {
        match self {
            OceanField::Leveled(f) => &f.time,
            OceanField::Surface(f) => &f.time,
        }
    }
}

fn indices_in_range(vals: &[f64], lo: f64, hi: f64) -> Vec<usize> {
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    vals.iter()
        .enumerate()
        .filter(|(_, v)| **v >= lo && **v <= hi)
        .map(|(i, _)| i)
        .collect()
}

/// Index of the nearest value to `target`; ties prefer the later index (see
/// the identical helper in `query.rs` for why).
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

impl Field3 {
    pub fn select_region(&self, region: &BoundingBox) -> Field3 {
        let lat_idx = indices_in_range(&self.lat, region.lat_min, region.lat_max);
        let lon_idx = indices_in_range(&self.lon, region.lon_min, region.lon_max);
        let data = self
            .data
            .select(Axis(1), &lat_idx)
            .select(Axis(2), &lon_idx);
        Field3 {
            time: self.time.clone(),
            lat: lat_idx.iter().map(|&i| self.lat[i]).collect(),
            lon: lon_idx.iter().map(|&i| self.lon[i]).collect(),
            data,
            long_name: self.long_name.clone(),
            units: self.units.clone(),
        }
    }

    /// Mean over the lat/lon (spatial) axes -> one value per time step.
    pub fn spatial_mean(&self) -> Vec<f64> {
        (0..self.time.len())
            .map(|t| {
                let slice = self.data.index_axis(Axis(0), t);
                let (sum, count) = slice.iter().fold((0.0, 0usize), |(s, c), v| {
                    if v.is_finite() {
                        (s + v, c + 1)
                    } else {
                        (s, c)
                    }
                });
                if count == 0 {
                    f64::NAN
                } else {
                    sum / count as f64
                }
            })
            .collect()
    }
}

impl Field4 {
    pub fn select_region(&self, region: &BoundingBox) -> Field4 {
        let lat_idx = indices_in_range(&self.lat, region.lat_min, region.lat_max);
        let lon_idx = indices_in_range(&self.lon, region.lon_min, region.lon_max);
        let data = self
            .data
            .select(Axis(2), &lat_idx)
            .select(Axis(3), &lon_idx);
        Field4 {
            time: self.time.clone(),
            level: self.level.clone(),
            lat: lat_idx.iter().map(|&i| self.lat[i]).collect(),
            lon: lon_idx.iter().map(|&i| self.lon[i]).collect(),
            data,
            long_name: self.long_name.clone(),
            units: self.units.clone(),
        }
    }

    /// Select the nearest depth level, dropping the `level` dimension.
    pub fn select_depth(&self, depth_m: f64) -> Field3 {
        let idx = nearest_index(&self.level, depth_m);
        let data = self.data.index_axis(Axis(1), idx).to_owned();
        Field3 {
            time: self.time.clone(),
            lat: self.lat.clone(),
            lon: self.lon.clone(),
            data,
            long_name: self.long_name.clone(),
            units: self.units.clone(),
        }
    }
}

/// Apply fill-value masking and unit conversion for a GODAS variable.
///
/// Mirrors `ocean._mask_and_convert`.
fn mask_and_convert<D: ndarray::Dimension>(
    data: &ndarray::Array<f64, D>,
    var: &str,
    fill_value: Option<f64>,
) -> ndarray::Array<f64, D> {
    let info = GODAS_VARS[var];
    data.mapv(|mut x| {
        if let Some(fv) = fill_value {
            if x == fv {
                x = f64::NAN;
            }
        }
        if let Some(vmin) = info.valid_min {
            // NaN must also be masked here (as `!(x > vmin)` would do): a
            // plain `x <= vmin` is false for NaN under IEEE 754, which
            // would let already-NaN fill values slip through unmasked.
            if x.is_nan() || x <= vmin {
                x = f64::NAN;
            }
        }
        if matches!(var, "ucur" | "vcur") && x.abs() >= 100.0 {
            x = f64::NAN;
        }
        match var {
            "pottmp" => x - K_TO_C,
            "salt" => x * KGK_TO_PSU,
            _ => x,
        }
    })
}

// ══════════════════════════════════════════════════════════════════════════
// Raw (unprocessed) data, as it would come straight off OPeNDAP
// ══════════════════════════════════════════════════════════════════════════

/// Unmasked, unconverted GODAS data straight off OPeNDAP: the Rust
/// equivalent of the synthetic `xr.Dataset` built by
/// `tests/test_ocean.py::_make_godas_ds`.
#[derive(Debug, Clone)]
pub enum RawGodas {
    Leveled {
        time: Vec<NaiveDate>,
        level: Vec<f64>,
        lat: Vec<f64>,
        lon: Vec<f64>,
        data: Array4<f64>,
        fill_value: Option<f64>,
    },
    Surface {
        time: Vec<NaiveDate>,
        lat: Vec<f64>,
        lon: Vec<f64>,
        data: Array3<f64>,
        fill_value: Option<f64>,
    },
}

/// Apply masking, unit conversion, depth selection and region cropping to
/// raw GODAS data. Pure and always compiled: this is the part of
/// `open_godas` that `tests/test_ocean.py` actually exercises (network I/O
/// is mocked out in every Python test).
///
/// Mirrors `ocean.open_godas` (minus the `xr.open_dataset` call).
pub fn godas_from_raw(
    raw: RawGodas,
    variable: &str,
    depth_m: Option<f64>,
    region: Option<BoundingBox>,
) -> Result<OceanField> {
    let info = *GODAS_VARS
        .get(variable)
        .ok_or_else(|| Error::UnknownGodasVariable(variable.to_string()))?;

    let mut field = match raw {
        RawGodas::Leveled {
            time,
            level,
            lat,
            lon,
            data,
            fill_value,
        } => OceanField::Leveled(Field4 {
            time,
            level,
            lat,
            lon,
            data: mask_and_convert(&data, variable, fill_value),
            long_name: info.long_name.to_string(),
            units: info.units_out.to_string(),
        }),
        RawGodas::Surface {
            time,
            lat,
            lon,
            data,
            fill_value,
        } => OceanField::Surface(Field3 {
            time,
            lat,
            lon,
            data: mask_and_convert(&data, variable, fill_value),
            long_name: info.long_name.to_string(),
            units: info.units_out.to_string(),
        }),
    };

    if info.has_levels {
        if let (OceanField::Leveled(f4), Some(depth)) = (&field, depth_m) {
            field = OceanField::Surface(f4.select_depth(depth));
        }
    }

    if let Some(region) = region {
        field = match field {
            OceanField::Leveled(f4) => OceanField::Leveled(f4.select_region(&region)),
            OceanField::Surface(f3) => OceanField::Surface(f3.select_region(&region)),
        };
    }

    Ok(field)
}

/// Concatenate several yearly [`OceanField`]s along the time axis. Mirrors
/// `ocean._concat_years` (generalised over both field shapes).
pub fn concat_years(mut pieces: Vec<OceanField>) -> Result<OceanField> {
    if pieces.is_empty() {
        return Err(Error::other("no pieces to concatenate"));
    }
    if pieces.len() == 1 {
        return Ok(pieces.remove(0));
    }
    let all_surface = pieces.iter().all(|p| matches!(p, OceanField::Surface(_)));
    if all_surface {
        let fields: Vec<Field3> = pieces
            .into_iter()
            .map(|p| match p {
                OceanField::Surface(f) => f,
                _ => unreachable!(),
            })
            .collect();
        let time: Vec<NaiveDate> = fields.iter().flat_map(|f| f.time.clone()).collect();
        let views: Vec<_> = fields.iter().map(|f| f.data.view()).collect();
        let data =
            ndarray::concatenate(Axis(0), &views).map_err(|e| Error::other(e.to_string()))?;
        Ok(OceanField::Surface(Field3 {
            time,
            lat: fields[0].lat.clone(),
            lon: fields[0].lon.clone(),
            data,
            long_name: fields[0].long_name.clone(),
            units: fields[0].units.clone(),
        }))
    } else {
        let fields: Vec<Field4> = pieces
            .into_iter()
            .map(|p| match p {
                OceanField::Leveled(f) => f,
                _ => unreachable!("mixed leveled/surface fields cannot be concatenated"),
            })
            .collect();
        let time: Vec<NaiveDate> = fields.iter().flat_map(|f| f.time.clone()).collect();
        let views: Vec<_> = fields.iter().map(|f| f.data.view()).collect();
        let data =
            ndarray::concatenate(Axis(0), &views).map_err(|e| Error::other(e.to_string()))?;
        Ok(OceanField::Leveled(Field4 {
            time,
            level: fields[0].level.clone(),
            lat: fields[0].lat.clone(),
            lon: fields[0].lon.clone(),
            data,
            long_name: fields[0].long_name.clone(),
            units: fields[0].units.clone(),
        }))
    }
}

// ══════════════════════════════════════════════════════════════════════════
// Time series
// ══════════════════════════════════════════════════════════════════════════

/// A monthly time series: the Rust analogue of a `pd.Series` with a
/// `DatetimeIndex`.
#[derive(Debug, Clone)]
pub struct TimeSeries {
    pub index: Vec<NaiveDate>,
    pub values: Vec<f64>,
    pub name: String,
}

impl TimeSeries {
    pub fn new(index: Vec<NaiveDate>, values: Vec<f64>, name: impl Into<String>) -> Self {
        TimeSeries {
            index,
            values,
            name: name.into(),
        }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// NaN-skipping mean.
    pub fn mean(&self) -> f64 {
        let finite: Vec<f64> = self
            .values
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        if finite.is_empty() {
            f64::NAN
        } else {
            finite.iter().sum::<f64>() / finite.len() as f64
        }
    }

    /// NaN-skipping population std-dev.
    pub fn std(&self) -> f64 {
        let finite: Vec<f64> = self
            .values
            .iter()
            .copied()
            .filter(|v| v.is_finite())
            .collect();
        if finite.len() < 2 {
            return f64::NAN;
        }
        let m = finite.iter().sum::<f64>() / finite.len() as f64;
        let var = finite.iter().map(|v| (v - m).powi(2)).sum::<f64>() / (finite.len() as f64 - 1.0);
        var.sqrt()
    }

    /// Centered 3-sample rolling mean with `min_periods = 3`. Mirrors
    /// `.rolling(window=3, center=True, min_periods=3).mean()`.
    pub fn rolling_mean_centered3(&self, name: impl Into<String>) -> TimeSeries {
        let n = self.values.len();
        let values = (0..n)
            .map(|i| {
                if i == 0 || i + 1 >= n {
                    return f64::NAN;
                }
                let w = [self.values[i - 1], self.values[i], self.values[i + 1]];
                if w.iter().all(|v| v.is_finite()) {
                    w.iter().sum::<f64>() / 3.0
                } else {
                    f64::NAN
                }
            })
            .collect();
        TimeSeries::new(self.index.clone(), values, name)
    }

    /// Mean value per calendar month (1–12) over years in `[clim_start,
    /// clim_end]`. Mirrors the monthly-climatology groupby in
    /// `ocean.get_nino_anomaly`.
    fn monthly_climatology(&self, clim_start: i32, clim_end: i32) -> HashMap<u32, f64> {
        let mut sums: HashMap<u32, (f64, usize)> = HashMap::new();
        for (d, v) in self.index.iter().zip(&self.values) {
            let y = d.year();
            if y >= clim_start && y <= clim_end && v.is_finite() {
                let e = sums.entry(d.month()).or_insert((0.0, 0));
                e.0 += v;
                e.1 += 1;
            }
        }
        sums.into_iter()
            .map(|(m, (s, c))| (m, s / c as f64))
            .collect()
    }
}

/// SST anomaly relative to a `[clim_start, clim_end]` monthly climatology,
/// restricted to `[year_start, year_end]`. `sst` must already cover the
/// full `min(year_start, clim_start)..max(year_end, clim_end)` span.
///
/// Mirrors `ocean.get_nino_anomaly` (minus the network fetch).
pub fn nino_anomaly_from_series(
    sst: &TimeSeries,
    year_start: i32,
    year_end: i32,
    clim_start: i32,
    clim_end: i32,
) -> TimeSeries {
    let clim = sst.monthly_climatology(clim_start, clim_end);
    let mut index = Vec::new();
    let mut values = Vec::new();
    for (d, v) in sst.index.iter().zip(&sst.values) {
        if d.year() >= year_start && d.year() <= year_end {
            let mean = clim.get(&d.month()).copied().unwrap_or(f64::NAN);
            index.push(*d);
            values.push(v - mean);
        }
    }
    TimeSeries::new(index, values, format!("{}_anom", sst.name))
}

/// Oceanic Niño Index: 3-month centered running mean of the Niño 3.4
/// anomaly. Mirrors `ocean.get_oni` (minus the network fetch).
pub fn oni_from_series(
    sst_nino34: &TimeSeries,
    year_start: i32,
    year_end: i32,
    clim_start: i32,
    clim_end: i32,
) -> TimeSeries {
    let anom = nino_anomaly_from_series(sst_nino34, year_start, year_end, clim_start, clim_end);
    anom.rolling_mean_centered3("ONI")
}

/// Default ONI threshold (°C) used by the CPC classification rule.
pub const DEFAULT_ENSO_THRESHOLD: f64 = 0.5;
/// Default minimum run length (overlapping 3-month seasons) required.
pub const DEFAULT_MIN_CONSECUTIVE: usize = 5;

/// Classify each month as `"El Niño"`, `"La Niña"`, or `"Neutral"`.
///
/// Follows the NOAA CPC ONI rule: the anomaly must exceed `threshold` for
/// at least `min_consecutive` consecutive months (`oni` is already a
/// 3-month running mean, so this reproduces "5 consecutive overlapping
/// seasons").
///
/// Mirrors `ocean.classify_enso` exactly (same run-length semantics).
pub fn classify_enso(oni: &TimeSeries, threshold: f64, min_consecutive: usize) -> Vec<String> {
    let n = oni.values.len();
    let mut raw = vec!["Neutral".to_string(); n];
    for (phase, sign) in [("El Niño", 1.0_f64), ("La Niña", -1.0_f64)] {
        let cond: Vec<bool> = oni
            .values
            .iter()
            .map(|v| v.is_finite() && sign * v >= threshold)
            .collect();
        let mut i = 0;
        while i < n {
            if cond[i] {
                let start = i;
                while i < n && cond[i] {
                    i += 1;
                }
                if i - start >= min_consecutive {
                    for r in raw.iter_mut().take(i).skip(start) {
                        *r = phase.to_string();
                    }
                }
            } else {
                i += 1;
            }
        }
    }
    raw
}

/// One row of [`enso_summary_from_series`].
#[derive(Debug, Clone)]
pub struct EnsoSummaryRow {
    pub month: NaiveDate,
    pub sst_nino34: f64,
    pub anom_nino34: f64,
    pub oni: f64,
    pub phase: String,
}

/// Monthly ENSO diagnostics table (SST, anomaly, ONI, phase), built from an
/// already-fetched Niño 3.4 SST series. Mirrors `ocean.enso_summary` (minus
/// the network fetch).
pub fn enso_summary_from_series(
    sst: &TimeSeries,
    year_start: i32,
    year_end: i32,
    clim_start: i32,
    clim_end: i32,
) -> Vec<EnsoSummaryRow> {
    let anom = nino_anomaly_from_series(sst, year_start, year_end, clim_start, clim_end);
    let oni = oni_from_series(sst, year_start, year_end, clim_start, clim_end);
    let phase = classify_enso(&oni, DEFAULT_ENSO_THRESHOLD, DEFAULT_MIN_CONSECUTIVE);

    let sst_by_month: HashMap<NaiveDate, f64> = sst
        .index
        .iter()
        .zip(&sst.values)
        .filter(|(d, _)| d.year() >= year_start && d.year() <= year_end)
        .map(|(d, v)| (*d, *v))
        .collect();

    oni.index
        .iter()
        .zip(&oni.values)
        .zip(&anom.values)
        .zip(&phase)
        .map(|(((d, o), a), p)| EnsoSummaryRow {
            month: *d,
            sst_nino34: sst_by_month.get(d).copied().unwrap_or(f64::NAN),
            anom_nino34: *a,
            oni: *o,
            phase: p.clone(),
        })
        .collect()
}

// ══════════════════════════════════════════════════════════════════════════
// D20 thermocline depth & Warm Water Volume (pure, operate on Field4)
// ══════════════════════════════════════════════════════════════════════════

/// Depth (m) of the `isotherm_temp` °C isotherm: the D20 index, computed
/// from an already-fetched, region-cropped `pottmp` field (°C).
///
/// Mirrors `ocean.get_thermocline_depth` (minus the network fetch, and
/// operating on a single year's field rather than looping years. Call this
/// once per year and concatenate with [`Field3::select_region`] /
/// `ndarray::concatenate` as needed, exactly like `get_thermocline_depth`
/// does internally in Python).
pub fn thermocline_depth_from_field4(pottmp: &Field4, isotherm_temp: f64) -> Field3 {
    let (nt, _nl, nlat, nlon) = pottmp.data.dim();
    let mut out = Array3::<f64>::from_elem((nt, nlat, nlon), f64::NAN);
    for t in 0..nt {
        for la in 0..nlat {
            for lo in 0..nlon {
                // Deepest level index that is still above the isotherm.
                let above_count = (0..pottmp.level.len())
                    .filter(|&l| pottmp.data[[t, l, la, lo]] > isotherm_temp)
                    .count();
                let idx = above_count.saturating_sub(1).min(pottmp.level.len() - 1);
                out[[t, la, lo]] = pottmp.level[idx];
            }
        }
    }
    Field3 {
        time: pottmp.time.clone(),
        lat: pottmp.lat.clone(),
        lon: pottmp.lon.clone(),
        data: out,
        long_name: format!("Depth of {isotherm_temp} °C isotherm (D20)"),
        units: "m".to_string(),
    }
}

/// Monthly Warm Water Volume (×10¹⁴ m³) in the equatorial Pacific, computed
/// from an already-fetched, WWV-box-cropped `pottmp` field (°C).
///
/// Mirrors `ocean.get_warm_water_volume` (minus the network fetch).
pub fn warm_water_volume_from_field4(
    pottmp: &Field4,
    temp_threshold: f64,
    max_depth: f64,
) -> TimeSeries {
    let level_idx: Vec<usize> = pottmp
        .level
        .iter()
        .enumerate()
        .filter(|(_, &l)| l <= max_depth)
        .map(|(i, _)| i)
        .collect();

    let dlev: Vec<f64> = gradient(
        &level_idx
            .iter()
            .map(|&i| pottmp.level[i])
            .collect::<Vec<_>>(),
    );
    let dlat: Vec<f64> = gradient(&pottmp.lat)
        .iter()
        .map(|v| v.abs() * 111_000.0)
        .collect();
    let dlon: Vec<f64> = gradient(&pottmp.lon)
        .iter()
        .enumerate()
        .map(|(i, v)| v.abs() * 111_000.0 * pottmp.lat[i].to_radians().cos())
        .collect();

    let (nt, _, _nlat, _nlon) = pottmp.data.dim();
    let mut values = Vec::with_capacity(nt);
    for t in 0..nt {
        let mut total = 0.0;
        for (li, &l) in level_idx.iter().enumerate() {
            for (la, &dlat_la) in dlat.iter().enumerate() {
                for (lo, &dlon_lo) in dlon.iter().enumerate() {
                    if pottmp.data[[t, l, la, lo]] > temp_threshold {
                        total += dlev[li] * dlat_la * dlon_lo;
                    }
                }
            }
        }
        values.push(total / 1e14);
    }

    TimeSeries::new(pottmp.time.clone(), values, "WWV_1e14m3")
}

/// NumPy-style `np.gradient` for a 1-D axis (central differences inside,
/// one-sided at the edges).
fn gradient(vals: &[f64]) -> Vec<f64> {
    let n = vals.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![0.0];
    }
    let mut out = vec![0.0; n];
    out[0] = vals[1] - vals[0];
    out[n - 1] = vals[n - 1] - vals[n - 2];
    for i in 1..n - 1 {
        out[i] = (vals[i + 1] - vals[i - 1]) / 2.0;
    }
    out
}

#[allow(dead_code)] // used under the `netcdf-io` feature
pub(crate) fn wwv_box() -> BoundingBox {
    BoundingBox::new(WWV_BOX.lat.0, WWV_BOX.lat.1, WWV_BOX.lon.0, WWV_BOX.lon.1)
}

/// Ocean current components plus derived speed. Returned by [`get_currents`].
/// Always defined (regardless of the `netcdf-io` feature) so the type is
/// nameable from downstream code either way.
#[derive(Debug, Clone)]
pub struct OceanCurrents {
    pub ucur: Field3,
    pub vcur: Field3,
    pub speed: Field3,
}

// ══════════════════════════════════════════════════════════════════════════
// Live OPeNDAP access (requires the `netcdf-io` feature)
// ══════════════════════════════════════════════════════════════════════════

#[cfg(feature = "netcdf-io")]
mod live {
    use super::*;

    const GODAS_BASE: &str = "https://psl.noaa.gov/thredds/dodsC/Datasets/godas/{var}.{year}.nc";
    const ERSST_URL: &str =
        "https://psl.noaa.gov/thredds/dodsC/Datasets/noaa.ersst.v5/sst.mnmean.nc";

    fn nc_f64(var: &netcdf::Variable) -> Result<Vec<f64>> {
        var.get_values::<f64, _>(..)
            .map_err(|e| Error::other(e.to_string()))
    }

    fn nc_fill_value(var: &netcdf::Variable) -> Option<f64> {
        var.attribute("missing_value")
            .or_else(|| var.attribute("_FillValue"))
            .and_then(|a| a.value().ok())
            .and_then(|v| format!("{v:?}").parse::<f64>().ok())
    }

    fn months_since_epoch(n: usize, start_year: i32) -> Vec<NaiveDate> {
        (0..n)
            .map(|i| {
                let y = start_year + (i as i32) / 12;
                let m = (i as i32) % 12 + 1;
                NaiveDate::from_ymd_opt(y, m as u32, 1).unwrap()
            })
            .collect()
    }

    /// Open one year of GODAS data via OPeNDAP (lazy handle, eager read).
    /// Requires a `libnetcdf` built with DAP/curl support.
    ///
    /// Mirrors `ocean.open_godas`.
    pub fn open_godas(
        year: i32,
        variable: &str,
        depth_m: Option<f64>,
        region: Option<BoundingBox>,
    ) -> Result<OceanField> {
        let info = *GODAS_VARS
            .get(variable)
            .ok_or_else(|| Error::UnknownGodasVariable(variable.to_string()))?;
        let url = GODAS_BASE
            .replace("{var}", variable)
            .replace("{year}", &year.to_string());
        let file = netcdf::open(&url).map_err(|e| Error::other(e.to_string()))?;
        let var = file
            .variable(variable)
            .ok_or_else(|| Error::other(format!("variable '{variable}' not found in {url}")))?;
        let fill_value = nc_fill_value(&var);

        let lat = nc_f64(
            &file
                .variable("lat")
                .ok_or_else(|| Error::other("missing 'lat'"))?,
        )?;
        let lon = nc_f64(
            &file
                .variable("lon")
                .ok_or_else(|| Error::other("missing 'lon'"))?,
        )?;
        let n_time = 12usize; // GODAS files are one calendar year, monthly.
        let time = months_since_epoch(n_time, year);

        let raw = if info.has_levels {
            let level = nc_f64(
                &file
                    .variable("level")
                    .ok_or_else(|| Error::other("missing 'level'"))?,
            )?;
            let flat = nc_f64(&var)?;
            let data = Array4::from_shape_vec((n_time, level.len(), lat.len(), lon.len()), flat)
                .map_err(|e| Error::other(e.to_string()))?;
            RawGodas::Leveled {
                time,
                level,
                lat,
                lon,
                data,
                fill_value,
            }
        } else {
            let flat = nc_f64(&var)?;
            let data = Array3::from_shape_vec((n_time, lat.len(), lon.len()), flat)
                .map_err(|e| Error::other(e.to_string()))?;
            RawGodas::Surface {
                time,
                lat,
                lon,
                data,
                fill_value,
            }
        };

        godas_from_raw(raw, variable, depth_m, region)
    }

    /// Multi-year GODAS field. Mirrors `ocean.get_godas`.
    pub fn get_godas(
        year_start: i32,
        year_end: i32,
        variable: &str,
        depth_m: Option<f64>,
        region: Option<BoundingBox>,
    ) -> Result<OceanField> {
        let mut pieces = Vec::new();
        for yr in year_start..=year_end {
            match open_godas(yr, variable, depth_m, region) {
                Ok(f) => pieces.push(f),
                Err(e) => eprintln!("[noawclg] could not load GODAS {variable} {yr}: {e}"),
            }
        }
        if pieces.is_empty() {
            return Err(Error::NoDataForRange {
                start: year_start,
                end: year_end,
                reason: format!("no GODAS '{variable}' data loaded"),
            });
        }
        concat_years(pieces)
    }

    pub fn get_ocean_temp(
        year_start: i32,
        year_end: i32,
        depth_m: f64,
        region: Option<BoundingBox>,
    ) -> Result<Field3> {
        get_godas(year_start, year_end, "pottmp", Some(depth_m), region)?.into_surface()
    }

    pub fn get_salinity(
        year_start: i32,
        year_end: i32,
        depth_m: f64,
        region: Option<BoundingBox>,
    ) -> Result<Field3> {
        get_godas(year_start, year_end, "salt", Some(depth_m), region)?.into_surface()
    }

    pub fn get_currents(
        year_start: i32,
        year_end: i32,
        depth_m: f64,
        region: Option<BoundingBox>,
    ) -> Result<OceanCurrents> {
        let u = get_godas(year_start, year_end, "ucur", Some(depth_m), region)?.into_surface()?;
        let v = get_godas(year_start, year_end, "vcur", Some(depth_m), region)?.into_surface()?;
        let mut speed = u.clone();
        speed.data = (&u.data * &u.data + &v.data * &v.data).mapv(f64::sqrt);
        speed.long_name = "Ocean current speed".to_string();
        speed.units = "m/s".to_string();
        Ok(OceanCurrents {
            ucur: u,
            vcur: v,
            speed,
        })
    }

    pub fn get_ssh(year_start: i32, year_end: i32, region: Option<BoundingBox>) -> Result<Field3> {
        get_godas(year_start, year_end, "sshg", None, region)?.into_surface()
    }

    /// Open NOAA ERSST v5 via OPeNDAP. Mirrors `ocean.open_ersst`.
    pub fn open_ersst(
        year_start: Option<i32>,
        year_end: Option<i32>,
        region: Option<BoundingBox>,
    ) -> Result<Field3> {
        let file = netcdf::open(ERSST_URL).map_err(|e| Error::other(e.to_string()))?;
        let var = file
            .variable("sst")
            .ok_or_else(|| Error::other("missing 'sst'"))?;
        let fill_value = nc_fill_value(&var);
        let lat = nc_f64(
            &file
                .variable("lat")
                .ok_or_else(|| Error::other("missing 'lat'"))?,
        )?;
        let lon = nc_f64(
            &file
                .variable("lon")
                .ok_or_else(|| Error::other("missing 'lon'"))?,
        )?;
        let flat = nc_f64(&var)?;
        let n_time = flat.len() / (lat.len() * lon.len());
        let data = Array3::from_shape_vec((n_time, lat.len(), lon.len()), flat)
            .map_err(|e| Error::other(e.to_string()))?;
        // ERSST starts 1854-01.
        let time = months_since_epoch(n_time, 1854);

        let mut field = Field3 {
            time,
            lat,
            lon,
            data: data.mapv(|mut x| {
                if let Some(fv) = fill_value {
                    if x == fv {
                        x = f64::NAN;
                    }
                }
                if x.abs() >= 100.0 {
                    x = f64::NAN;
                }
                x
            }),
            long_name: "Sea Surface Temperature".to_string(),
            units: "°C".to_string(),
        };

        if let (Some(y0), Some(y1)) = (year_start, year_end) {
            let keep: Vec<usize> = field
                .time
                .iter()
                .enumerate()
                .filter(|(_, d)| d.year() >= y0 && d.year() <= y1)
                .map(|(i, _)| i)
                .collect();
            field.data = field.data.select(Axis(0), &keep);
            field.time = keep.iter().map(|&i| field.time[i]).collect();
        }
        if let Some(region) = region {
            field = field.select_region(&region);
        }
        Ok(field)
    }

    /// Monthly mean SST averaged over a standard Niño box. Mirrors
    /// `ocean.get_sst_series`.
    pub fn get_sst_series(
        year_start: i32,
        year_end: i32,
        boxname: &str,
        source: &str,
    ) -> Result<TimeSeries> {
        let b = *NINO_BOXES
            .get(boxname)
            .ok_or_else(|| Error::other(format!("unknown Nino box '{boxname}'")))?;
        let region = BoundingBox::new(b.lat.0, b.lat.1, b.lon.0, b.lon.1);
        let field = if source == "ersst" {
            open_ersst(Some(year_start), Some(year_end), Some(region))?
        } else {
            get_ocean_temp(year_start, year_end, 5.0, Some(region))?
        };
        Ok(TimeSeries::new(
            field.time.clone(),
            field.spatial_mean(),
            format!("SST_Nino{boxname}"),
        ))
    }

    pub fn get_nino_anomaly(
        year_start: i32,
        year_end: i32,
        boxname: &str,
        clim_start: i32,
        clim_end: i32,
        source: &str,
    ) -> Result<TimeSeries> {
        let all_start = year_start.min(clim_start);
        let all_end = year_end.max(clim_end);
        let sst = get_sst_series(all_start, all_end, boxname, source)?;
        Ok(nino_anomaly_from_series(
            &sst, year_start, year_end, clim_start, clim_end,
        ))
    }

    pub fn get_oni(
        year_start: i32,
        year_end: i32,
        clim_start: i32,
        clim_end: i32,
        source: &str,
    ) -> Result<TimeSeries> {
        let all_start = year_start.min(clim_start);
        let all_end = year_end.max(clim_end);
        let sst = get_sst_series(all_start, all_end, "3.4", source)?;
        Ok(oni_from_series(
            &sst, year_start, year_end, clim_start, clim_end,
        ))
    }

    pub fn get_thermocline_depth(
        year_start: i32,
        year_end: i32,
        region: Option<BoundingBox>,
        isotherm_temp: f64,
    ) -> Result<Field3> {
        let mut pieces = Vec::new();
        for yr in year_start..=year_end {
            let field = open_godas(yr, "pottmp", None, region)?;
            if let OceanField::Leveled(f4) = field {
                pieces.push(thermocline_depth_from_field4(&f4, isotherm_temp));
            }
        }
        if pieces.is_empty() {
            return Err(Error::NoDataForRange {
                start: year_start,
                end: year_end,
                reason: "no D20 computed".into(),
            });
        }
        let time: Vec<NaiveDate> = pieces.iter().flat_map(|f| f.time.clone()).collect();
        let views: Vec<_> = pieces.iter().map(|f| f.data.view()).collect();
        let data =
            ndarray::concatenate(Axis(0), &views).map_err(|e| Error::other(e.to_string()))?;
        Ok(Field3 {
            time,
            lat: pieces[0].lat.clone(),
            lon: pieces[0].lon.clone(),
            data,
            long_name: pieces[0].long_name.clone(),
            units: "m".to_string(),
        })
    }

    pub fn get_warm_water_volume(
        year_start: i32,
        year_end: i32,
        temp_threshold: f64,
        max_depth: f64,
    ) -> Result<TimeSeries> {
        let mut all_index = Vec::new();
        let mut all_values = Vec::new();
        for yr in year_start..=year_end {
            match open_godas(yr, "pottmp", None, Some(wwv_box())) {
                Ok(OceanField::Leveled(f4)) => {
                    let ts = warm_water_volume_from_field4(&f4, temp_threshold, max_depth);
                    all_index.extend(ts.index);
                    all_values.extend(ts.values);
                }
                Ok(_) => {}
                Err(e) => eprintln!("[noawclg] could not compute WWV for {yr}: {e}"),
            }
        }
        if all_index.is_empty() {
            return Err(Error::NoDataForRange {
                start: year_start,
                end: year_end,
                reason: "no WWV computed".into(),
            });
        }
        Ok(TimeSeries::new(all_index, all_values, "WWV_1e14m3"))
    }

    pub fn enso_summary(
        year_start: i32,
        year_end: i32,
        clim_start: i32,
        clim_end: i32,
        source: &str,
    ) -> Result<Vec<EnsoSummaryRow>> {
        let all_start = year_start.min(clim_start);
        let all_end = year_end.max(clim_end);
        let sst = get_sst_series(all_start, all_end, "3.4", source)?;
        Ok(enso_summary_from_series(
            &sst, year_start, year_end, clim_start, clim_end,
        ))
    }
}

#[cfg(feature = "netcdf-io")]
pub use live::*;

// ══════════════════════════════════════════════════════════════════════════
// Stubs for when `netcdf-io` is not compiled in: same signatures as
// `live`, so downstream code (and this crate's own README examples) can be
// written once and fail predictably at runtime rather than not compiling
// at all when the feature is off. Mirrors the `grib`/`not(grib)` split in
// `gfs_dataset.rs`.
// ══════════════════════════════════════════════════════════════════════════

#[cfg(not(feature = "netcdf-io"))]
mod live_stub {
    use super::*;

    const DISABLED: Error = Error::FeatureDisabled("netcdf-io");

    pub fn open_godas(
        _year: i32,
        _variable: &str,
        _depth_m: Option<f64>,
        _region: Option<BoundingBox>,
    ) -> Result<OceanField> {
        Err(DISABLED)
    }

    pub fn get_godas(
        _year_start: i32,
        _year_end: i32,
        _variable: &str,
        _depth_m: Option<f64>,
        _region: Option<BoundingBox>,
    ) -> Result<OceanField> {
        Err(DISABLED)
    }

    pub fn get_ocean_temp(
        _year_start: i32,
        _year_end: i32,
        _depth_m: f64,
        _region: Option<BoundingBox>,
    ) -> Result<Field3> {
        Err(DISABLED)
    }

    pub fn get_salinity(
        _year_start: i32,
        _year_end: i32,
        _depth_m: f64,
        _region: Option<BoundingBox>,
    ) -> Result<Field3> {
        Err(DISABLED)
    }

    pub fn get_currents(
        _year_start: i32,
        _year_end: i32,
        _depth_m: f64,
        _region: Option<BoundingBox>,
    ) -> Result<OceanCurrents> {
        Err(DISABLED)
    }

    pub fn get_ssh(
        _year_start: i32,
        _year_end: i32,
        _region: Option<BoundingBox>,
    ) -> Result<Field3> {
        Err(DISABLED)
    }

    pub fn open_ersst(
        _year_start: Option<i32>,
        _year_end: Option<i32>,
        _region: Option<BoundingBox>,
    ) -> Result<Field3> {
        Err(DISABLED)
    }

    pub fn get_sst_series(
        _year_start: i32,
        _year_end: i32,
        _boxname: &str,
        _source: &str,
    ) -> Result<TimeSeries> {
        Err(DISABLED)
    }

    pub fn get_nino_anomaly(
        _year_start: i32,
        _year_end: i32,
        _boxname: &str,
        _clim_start: i32,
        _clim_end: i32,
        _source: &str,
    ) -> Result<TimeSeries> {
        Err(DISABLED)
    }

    pub fn get_oni(
        _year_start: i32,
        _year_end: i32,
        _clim_start: i32,
        _clim_end: i32,
        _source: &str,
    ) -> Result<TimeSeries> {
        Err(DISABLED)
    }

    pub fn get_thermocline_depth(
        _year_start: i32,
        _year_end: i32,
        _region: Option<BoundingBox>,
        _isotherm_temp: f64,
    ) -> Result<Field3> {
        Err(DISABLED)
    }

    pub fn get_warm_water_volume(
        _year_start: i32,
        _year_end: i32,
        _temp_threshold: f64,
        _max_depth: f64,
    ) -> Result<TimeSeries> {
        Err(DISABLED)
    }

    pub fn enso_summary(
        _year_start: i32,
        _year_end: i32,
        _clim_start: i32,
        _clim_end: i32,
        _source: &str,
    ) -> Result<Vec<EnsoSummaryRow>> {
        Err(DISABLED)
    }
}

#[cfg(not(feature = "netcdf-io"))]
pub use live_stub::*;

#[cfg(test)]
mod tests {
    use super::*;

    fn synth_pottmp(n_levels: bool) -> RawGodas {
        let time: Vec<NaiveDate> = (1..=12)
            .map(|m| NaiveDate::from_ymd_opt(2024, m, 1).unwrap())
            .collect();
        let lat: Vec<f64> = (0..21).map(|i| -10.0 + i as f64).collect();
        let lon: Vec<f64> = (0..29).map(|i| 150.0 + i as f64 * 5.0).collect();
        if n_levels {
            let level = GODAS_LEVELS[..8].to_vec();
            let shape = (time.len(), level.len(), lat.len(), lon.len());
            let data = Array4::from_shape_fn(shape, |(t, l, la, lo)| {
                295.0 + (t + l + la + lo) as f64 % 10.0
            });
            RawGodas::Leveled {
                time,
                level,
                lat,
                lon,
                data,
                fill_value: Some(9.969_209_968_386_869e36),
            }
        } else {
            let shape = (time.len(), lat.len(), lon.len());
            let data = Array3::from_shape_fn(shape, |(t, la, lo)| {
                0.1 * ((t + la + lo) as f64 % 6.0 - 3.0)
            });
            RawGodas::Surface {
                time,
                lat,
                lon,
                data,
                fill_value: Some(9.969_209_968_386_869e36),
            }
        }
    }

    #[test]
    fn godas_vars_has_five_entries() {
        let mut keys: Vec<&str> = GODAS_VARS.keys().copied().collect();
        keys.sort_unstable();
        assert_eq!(keys, vec!["pottmp", "salt", "sshg", "ucur", "vcur"]);
    }

    #[test]
    fn pottmp_has_levels_sshg_does_not() {
        assert!(GODAS_VARS["pottmp"].has_levels);
        assert!(!GODAS_VARS["sshg"].has_levels);
    }

    #[test]
    fn nino34_box_is_0_360_longitude() {
        let b = NINO_BOXES["3.4"];
        assert_eq!(b.lat, (-5.0, 5.0));
        assert!(180.0 < b.lon.0 && b.lon.0 < b.lon.1 && b.lon.1 < 360.0);
    }

    #[test]
    fn open_godas_converts_kelvin_to_celsius() {
        let raw = synth_pottmp(true);
        let field = godas_from_raw(raw, "pottmp", None, None).unwrap();
        let f4 = match field {
            OceanField::Leveled(f) => f,
            _ => panic!("expected leveled field"),
        };
        assert!(f4.data.iter().all(|v| *v < 60.0));
    }

    #[test]
    fn open_godas_rejects_unknown_variable() {
        let raw = synth_pottmp(true);
        let err = godas_from_raw(raw, "bogus", None, None).unwrap_err();
        assert!(matches!(err, Error::UnknownGodasVariable(_)));
    }

    #[test]
    fn open_godas_depth_selection_drops_level_dim() {
        let raw = synth_pottmp(true);
        let field = godas_from_raw(raw, "pottmp", Some(200.0), None).unwrap();
        assert!(matches!(field, OceanField::Surface(_)));
    }

    #[test]
    fn open_godas_region_crops_lat_lon() {
        let raw = synth_pottmp(true);
        let region = BoundingBox::new(-5.0, 5.0, 190.0, 240.0);
        let field = godas_from_raw(raw, "pottmp", Some(200.0), Some(region))
            .unwrap()
            .into_surface()
            .unwrap();
        assert!(field.lat.iter().all(|v| *v >= -5.0 && *v <= 5.0));
        assert!(field.lon.iter().all(|v| *v >= 190.0 && *v <= 240.0));
    }

    #[test]
    fn sshg_has_no_level_dim() {
        let raw = synth_pottmp(false);
        // reuse synth for a surface variable name that matches has_levels=false
        let field = godas_from_raw(raw, "sshg", None, None).unwrap();
        assert!(matches!(field, OceanField::Surface(_)));
    }

    fn synthetic_sst(mean: f64, amplitude: f64, n: usize) -> TimeSeries {
        let index: Vec<NaiveDate> = (0..n)
            .map(|i| NaiveDate::from_ymd_opt(2015 + i as i32 / 12, (i % 12) as u32 + 1, 1).unwrap())
            .collect();
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
    fn oni_smooths_relative_to_raw() {
        let sst = synthetic_sst(27.0, 1.5, 72);
        let oni = oni_from_series(&sst, 2015, 2019, 2015, 2019);
        assert!(oni.std() <= sst.std() + 0.1);
    }

    fn synthetic_oni_nino() -> TimeSeries {
        let index: Vec<NaiveDate> = (0..36)
            .map(|i| NaiveDate::from_ymd_opt(2015 + i / 12, (i % 12) as u32 + 1, 1).unwrap())
            .collect();
        let mut values = vec![0.0; 36];
        for v in values.iter_mut().take(14).skip(6) {
            *v = 0.8;
        }
        values[5] = 0.5;
        values[14] = 0.5;
        TimeSeries::new(index, values, "ONI")
    }

    fn synthetic_oni_nina() -> TimeSeries {
        let index: Vec<NaiveDate> = (0..24)
            .map(|i| NaiveDate::from_ymd_opt(2020 + i / 12, (i % 12) as u32 + 1, 1).unwrap())
            .collect();
        let mut values = vec![0.0; 24];
        for v in values.iter_mut().take(9).skip(3) {
            *v = -0.8;
        }
        TimeSeries::new(index, values, "ONI")
    }

    #[test]
    fn classify_enso_detects_el_nino() {
        let oni = synthetic_oni_nino();
        let phase = classify_enso(&oni, 0.5, 5);
        assert!(phase.contains(&"El Niño".to_string()));
    }

    #[test]
    fn classify_enso_detects_la_nina() {
        let oni = synthetic_oni_nina();
        let phase = classify_enso(&oni, 0.5, 5);
        assert!(phase.contains(&"La Niña".to_string()));
    }

    #[test]
    fn classify_enso_all_zero_is_neutral() {
        let index: Vec<NaiveDate> = (1..=12)
            .map(|m| NaiveDate::from_ymd_opt(2020, m, 1).unwrap())
            .collect();
        let oni = TimeSeries::new(index, vec![0.0; 12], "ONI");
        let phase = classify_enso(&oni, 0.5, 5);
        assert!(phase.iter().all(|p| p == "Neutral"));
    }

    #[test]
    fn classify_enso_short_spike_stays_neutral() {
        let index: Vec<NaiveDate> = (1..=12)
            .map(|m| NaiveDate::from_ymd_opt(2020, m, 1).unwrap())
            .collect();
        let mut values = vec![0.0; 12];
        values[3] = 0.8;
        values[4] = 0.8;
        values[5] = 0.8; // only 3 months, need >= 5
        let oni = TimeSeries::new(index, values, "ONI");
        let phase = classify_enso(&oni, 0.5, 5);
        assert!(!phase.contains(&"El Niño".to_string()));
    }

    #[test]
    fn classify_enso_same_length_as_input() {
        let oni = synthetic_oni_nino();
        let phase = classify_enso(&oni, 0.5, 5);
        assert_eq!(phase.len(), oni.len());
    }

    #[test]
    fn thermocline_depth_within_valid_range() {
        let raw = synth_pottmp(true);
        let field = godas_from_raw(raw, "pottmp", None, None).unwrap();
        let f4 = match field {
            OceanField::Leveled(f) => f,
            _ => panic!(),
        };
        let d20 = thermocline_depth_from_field4(&f4, 20.0);
        assert!(d20.data.iter().all(|v| *v <= 4500.0));
        assert_eq!(d20.data.dim(), (f4.time.len(), f4.lat.len(), f4.lon.len()));
    }

    #[test]
    fn enso_summary_has_expected_phases() {
        let sst = synthetic_sst(27.0, 0.3, 72);
        let rows = enso_summary_from_series(&sst, 2015, 2017, 2015, 2019);
        assert!(!rows.is_empty());
        for row in &rows {
            assert!(["El Niño", "La Niña", "Neutral"].contains(&row.phase.as_str()));
        }
    }
}
