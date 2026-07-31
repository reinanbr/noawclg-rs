//! Coordinate utilities: dimension detection, bbox, date/lon helpers.
//!
//! Direct port of `noawclg/coords.py`.

use chrono::{Datelike, Duration, NaiveDate, Timelike, Utc};

use crate::error::{Error, Result};

/// Candidate dimension names, ordered by preference (most common first).
pub const LAT_CANDIDATES: &[&str] = &["lat", "latitude", "y", "nav_lat", "rlat", "XLAT"];
pub const LON_CANDIDATES: &[&str] = &["lon", "longitude", "x", "nav_lon", "rlon", "XLONG"];
pub const TIME_CANDIDATES: &[&str] = &["time", "Time", "t", "forecast_hour", "step"];

/// Return the first candidate name present in `coords`.
///
/// Mirrors `noawclg.coords._find_dim`.
pub fn find_dim(
    coords: &[String],
    candidates: &'static [&'static str],
    label: &str,
) -> Result<String> {
    for name in candidates {
        if coords.iter().any(|c| c == name) {
            return Ok((*name).to_string());
        }
    }
    Err(Error::DimensionNotFound {
        label: label.to_string(),
        tried: candidates.to_vec(),
        available: coords.to_vec(),
    })
}

/// Axis-aligned bounding box of a dataset's spatial domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub lat_min: f64,
    pub lat_max: f64,
    pub lon_min: f64,
    pub lon_max: f64,
}

impl BoundingBox {
    pub fn new(lat_min: f64, lat_max: f64, lon_min: f64, lon_max: f64) -> Self {
        BoundingBox {
            lat_min,
            lat_max,
            lon_min,
            lon_max,
        }
    }

    /// True if `(lat, lon)` falls inside this box (inclusive).
    pub fn contains(&self, lat: f64, lon: f64) -> bool {
        self.lat_min <= lat && lat <= self.lat_max && self.lon_min <= lon && lon <= self.lon_max
    }
}

impl std::fmt::Display for BoundingBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "lat=[{:.2}, {:.2}], lon=[{:.2}, {:.2}]",
            self.lat_min, self.lat_max, self.lon_min, self.lon_max
        )
    }
}

/// A `{toplat, bottomlat, leftlon, rightlon}` region as used by the NOMADS
/// grib-filter query string.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub toplat: f64,
    pub bottomlat: f64,
    pub leftlon: f64,
    pub rightlon: f64,
}

/// Convert `'DD/MM/YYYY'` → `'YYYYMMDD'`.
///
/// Mirrors `noawclg.coords._parse_date`.
pub fn parse_date(date: &str) -> Result<String> {
    let d = NaiveDate::parse_from_str(date, "%d/%m/%Y")
        .map_err(|_| Error::InvalidDate(date.to_string()))?;
    Ok(d.format("%Y%m%d").to_string())
}

/// Normalize `lon` to match the dataset's longitude convention.
///
/// Detects automatically whether the dataset uses `[0, 360]` or `[-180, 180]`.
///
/// Mirrors `noawclg.coords._normalize_lon`.
pub fn normalize_lon(lon: f64, lon_min: f64) -> f64 {
    if lon_min < 0.0 {
        (lon + 180.0).rem_euclid(360.0) - 180.0
    } else {
        lon.rem_euclid(360.0)
    }
}

/// Return `(date, cycle)` ready for `GFSDatasetManager`.
///
/// `date` is in `'DD/MM/YYYY'` format; `cycle` is one of `'00'`, `'06'`,
/// `'12'`, `'18'`. Using `lag_days = 1` targets yesterday's run, which is
/// always available on NOMADS.
///
/// Mirrors `noawclg.coords.auto_date`.
pub fn auto_date(lag_days: i64) -> (String, String) {
    let now = Utc::now();
    let run_date = now - Duration::days(lag_days);
    let available_hour = now.hour() as i64 - 4; // GFS takes ~4 h to publish
    let cycle = if available_hour >= 18 {
        "18"
    } else if available_hour >= 12 {
        "12"
    } else if available_hour >= 6 {
        "06"
    } else {
        "00"
    };
    let date_str = format!(
        "{:02}/{:02}/{:04}",
        run_date.day(),
        run_date.month(),
        run_date.year()
    );
    (date_str, cycle.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_br_format() {
        assert_eq!(parse_date("17/04/2026").unwrap(), "20260417");
    }

    #[test]
    fn parse_date_rejects_bad_format() {
        assert!(parse_date("2026-04-17").is_err());
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

    #[test]
    fn bounding_box_contains() {
        let b = BoundingBox::new(-10.0, 10.0, -80.0, -30.0);
        assert!(b.contains(-3.7, -38.5));
        assert!(!b.contains(20.0, -38.5));
    }
}
