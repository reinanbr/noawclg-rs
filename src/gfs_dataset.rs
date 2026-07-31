//! GFS Dataset Manager — download, parse, and assemble gridded datasets.
//!
//! Downloads one GRIB2 file per forecast hour via the NOMADS grib-filter
//! endpoint (all requested variables bundled in a single URL), then extracts
//! each variable and assembles a time-labelled [`GfsDataset`].
//!
//! Direct port of `noawclg/gfs_dataset.py`. GRIB2 decoding (`build_dataset`,
//! `build_multi_dataset`) requires the `grib` feature (pulls in
//! [`gribberish`]); without it, `download_hours` still works (raw GRIB2
//! files are downloaded and cached exactly like the Python version), but
//! the extraction step returns [`crate::Error::FeatureDisabled`].

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use ndarray::ArrayD;

use crate::catalog::VARIABLES;
use crate::coords::Region;
use crate::error::{Error, Result};
use crate::http::{self, Fetcher, ReqwestFetcher};

/// One decoded variable field: `data` dims follow `dims` exactly, in order
/// `["time", ("level",) "latitude", "longitude"]`.
#[derive(Debug, Clone)]
pub struct GfsVariable {
    pub data: ArrayD<f64>,
    pub dims: Vec<String>,
    pub long_name: String,
    pub units: String,
}

/// Gridded GFS dataset — the Rust analogue of the `xr.Dataset` returned by
/// the Python library's `build_dataset` / `build_multi_dataset`.
#[derive(Debug, Clone, Default)]
pub struct GfsDataset {
    pub time: Vec<DateTime<Utc>>,
    pub forecast_hour: Vec<u32>,
    pub latitude: Vec<f64>,
    pub longitude: Vec<f64>,
    pub level: Option<Vec<f64>>,
    pub var_order: Vec<String>,
    pub variables: std::collections::HashMap<String, GfsVariable>,
    pub attrs: std::collections::HashMap<String, String>,
}

impl GfsDataset {
    pub fn get(&self, key: &str) -> Option<&GfsVariable> {
        self.variables.get(key)
    }

    /// `{variable: long_name}` for every data variable, plus the coordinate
    /// variables — mirrors `get_noaa_data.get_keys()`.
    pub fn keys(&self) -> std::collections::HashMap<String, String> {
        let mut out = std::collections::HashMap::new();
        for name in &self.var_order {
            if let Some(v) = self.variables.get(name) {
                out.insert(name.clone(), v.long_name.clone());
            }
        }
        out
    }

    /// Coordinate list, used for `_find_dim`-style lookups.
    pub fn coord_names(&self) -> Vec<String> {
        let mut c = vec![
            "time".to_string(),
            "forecast_hour".to_string(),
            "latitude".to_string(),
            "longitude".to_string(),
        ];
        if self.level.is_some() {
            c.push("level".to_string());
        }
        c
    }

    /// Merge `other` into `self` (like `xr.merge(datasets, join="inner")`).
    /// Coordinates are taken from the first dataset merged.
    pub fn merge(mut self, other: GfsDataset) -> GfsDataset {
        for name in other.var_order {
            if let Some(v) = other.variables.get(&name) {
                if !self.variables.contains_key(&name) {
                    self.var_order.push(name.clone());
                    self.variables.insert(name, v.clone());
                }
            }
        }
        self
    }
}

/// Download GFS GRIB2 files (once per hour) and assemble [`GfsDataset`]s.
///
/// Mirrors `noawclg.gfs_dataset.GFSDatasetManager`.
pub struct GfsDatasetManager {
    pub date: String,
    pub cycle: String,
    pub output_dir: PathBuf,
    pub region: Option<Region>,
    pub pause: Duration,
    pub request_timeout: Duration,
    fetcher: Box<dyn Fetcher>,
    // Only read by `build_dataset`/`build_multi_dataset`, which are gated
    // behind the `grib` feature.
    #[allow(dead_code)]
    run_dt: NaiveDateTime,
}

// Manual impl: `Box<dyn Fetcher>` doesn't implement `Debug`, so `#[derive]`
// isn't an option; this is only needed so `Result<GfsDatasetManager, _>`
// satisfies `unwrap_err`'s `T: Debug` bound in tests.
impl std::fmt::Debug for GfsDatasetManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GfsDatasetManager")
            .field("date", &self.date)
            .field("cycle", &self.cycle)
            .field("output_dir", &self.output_dir)
            .field("region", &self.region)
            .finish_non_exhaustive()
    }
}

fn valid_cycle(cycle: &str) -> Result<()> {
    if matches!(cycle, "00" | "06" | "12" | "18") {
        Ok(())
    } else {
        Err(Error::InvalidCycle(cycle.to_string()))
    }
}

impl GfsDatasetManager {
    /// `date` must be in `YYYYMMDD` format (matches the Python constructor).
    pub fn new(
        date: &str,
        cycle: &str,
        output_dir: impl AsRef<Path>,
        region: Option<Region>,
    ) -> Result<Self> {
        Self::with_options(
            date,
            cycle,
            output_dir,
            region,
            Duration::from_millis(1500),
            Duration::from_secs(30),
        )
    }

    pub fn with_options(
        date: &str,
        cycle: &str,
        output_dir: impl AsRef<Path>,
        region: Option<Region>,
        pause: Duration,
        request_timeout: Duration,
    ) -> Result<Self> {
        let fetcher = ReqwestFetcher::new(request_timeout)?;
        Self::with_fetcher(
            date,
            cycle,
            output_dir,
            region,
            pause,
            request_timeout,
            Box::new(fetcher),
        )
    }

    /// Like [`Self::with_options`], but with the HTTP layer swapped out —
    /// used by the crate's integration tests (`tests/gfs_dataset_tests.rs`)
    /// to substitute a canned [`Fetcher`] instead of hitting NOMADS for
    /// real, the same way the Python test suite mocks
    /// `GFSDatasetManager._session.get`.
    pub fn with_fetcher(
        date: &str,
        cycle: &str,
        output_dir: impl AsRef<Path>,
        region: Option<Region>,
        pause: Duration,
        request_timeout: Duration,
        fetcher: Box<dyn Fetcher>,
    ) -> Result<Self> {
        valid_cycle(cycle)?;
        let run_date = chrono::NaiveDate::parse_from_str(date, "%Y%m%d")
            .map_err(|_| Error::InvalidDate(date.to_string()))?;
        let hour: u32 = cycle
            .parse()
            .map_err(|_| Error::InvalidCycle(cycle.to_string()))?;
        let run_dt = run_date
            .and_hms_opt(hour, 0, 0)
            .ok_or_else(|| Error::InvalidDate(date.to_string()))?;

        let output_dir = output_dir.as_ref().to_path_buf();
        fs::create_dir_all(&output_dir)?;

        Ok(GfsDatasetManager {
            date: date.to_string(),
            cycle: cycle.to_string(),
            output_dir,
            region,
            pause,
            request_timeout,
            fetcher,
            run_dt,
        })
    }

    fn region_params(&self) -> String {
        match &self.region {
            None => String::new(),
            Some(r) => format!(
                "&subregion=&toplat={}&bottomlat={}&leftlon={}&rightlon={}",
                r.toplat, r.bottomlat, r.leftlon, r.rightlon
            ),
        }
    }

    fn var_params(&self, var_keys: &[&str]) -> Result<String> {
        let mut seen = std::collections::HashSet::new();
        let mut parts = String::new();
        for vk in var_keys {
            let cfg = VARIABLES
                .get(vk)
                .ok_or_else(|| Error::UnknownVariables(vec![vk.to_string()]))?;
            let var_token = format!("&{}=on", cfg.grib_var);
            if seen.insert(var_token.clone()) {
                parts.push_str(&var_token);
            }
            if cfg.tlev == "isobaricInhPa" {
                if let Some(levels) = cfg.levels {
                    for lev in levels {
                        let lev_token = format!("&lev_{lev}_mb=on");
                        if seen.insert(lev_token.clone()) {
                            parts.push_str(&lev_token);
                        }
                    }
                }
            } else {
                let lev_token = format!("&{}=on", cfg.grib_lev);
                if seen.insert(lev_token.clone()) {
                    parts.push_str(&lev_token);
                }
            }
        }
        Ok(parts)
    }

    fn filter_url(&self, var_keys: &[&str], hour: u32) -> Result<String> {
        Ok(http::filter_url(
            &self.date,
            &self.cycle,
            hour,
            &self.var_params(var_keys)?,
            &self.region_params(),
        ))
    }

    fn cache_path(&self, var_keys: &[&str], hour: u32) -> PathBuf {
        let tag = match &self.region {
            None => "global".to_string(),
            Some(r) => format!(
                "{}N{}S{}W{}E",
                r.toplat,
                r.bottomlat.abs(),
                r.leftlon.abs(),
                r.rightlon
            ),
        };
        let mut sorted: Vec<&str> = var_keys.to_vec();
        sorted.sort_unstable();
        let mut vkey = sorted.join("_");
        vkey.truncate(60);
        self.output_dir.join(format!(
            "gfs_{}_{}z_{}_{}_f{:03}.grib2",
            self.date, self.cycle, vkey, tag, hour
        ))
    }

    fn is_valid_grib_file(path: &Path, min_size: u64) -> bool {
        let Ok(meta) = fs::metadata(path) else {
            return false;
        };
        if meta.len() < min_size {
            return false;
        }
        let Ok(bytes) = fs::read(path) else {
            return false;
        };
        if bytes.len() < 4 || &bytes[..4] != b"GRIB" {
            // Not a GRIB file at all (e.g. an HTML error page) — treat as
            // "present" the way the Python `_is_valid_grib_file` does for
            // non-GRIB content (it only checks the trailing marker when the
            // header matches).
            return true;
        }
        bytes.len() >= 4 && &bytes[bytes.len() - 4..] == b"7777"
    }

    /// Download one GRIB2 file per forecast hour containing all `var_keys`.
    /// Already-cached, valid files are skipped. Mirrors
    /// `GFSDatasetManager.download_hours`.
    pub fn download_hours(
        &self,
        var_keys: &[&str],
        hours: &[u32],
        force: bool,
    ) -> Result<BTreeMap<u32, PathBuf>> {
        let unknown: Vec<String> = var_keys
            .iter()
            .filter(|k| !VARIABLES.contains_key(*k))
            .map(|k| k.to_string())
            .collect();
        if !unknown.is_empty() {
            return Err(Error::UnknownVariables(unknown));
        }

        let mut results = BTreeMap::new();
        let mut to_fetch = Vec::new();

        for &hour in hours {
            let path = self.cache_path(var_keys, hour);
            if path.exists() && !force {
                if Self::is_valid_grib_file(&path, 100) {
                    results.insert(hour, path);
                    continue;
                }
                let _ = fs::remove_file(&path);
            }
            to_fetch.push(hour);
        }

        for hour in to_fetch {
            let path = self.cache_path(var_keys, hour);
            let url = self.filter_url(var_keys, hour)?;

            let mut attempt = 0u32;
            let outcome = loop {
                match self.fetcher.get(&url) {
                    Ok((status, body)) if (200..300).contains(&status) => break Some(body),
                    Ok((status, _))
                        if matches!(status, 429 | 500 | 502 | 503 | 504) && attempt < 2 =>
                    {
                        attempt += 1;
                        sleep(Duration::from_millis(
                            (500.0 * 2f64.powi(attempt as i32 - 1)) as u64,
                        ));
                        continue;
                    }
                    _ => break None,
                }
            };

            let Some(bytes) = outcome else {
                sleep(self.pause);
                continue;
            };

            if bytes.len() < 100 {
                sleep(self.pause);
                continue;
            }
            fs::write(&path, &bytes)?;
            if Self::is_valid_grib_file(&path, 100) {
                results.insert(hour, path);
            } else {
                let _ = fs::remove_file(&path);
            }
            sleep(self.pause);
        }

        Ok(results)
    }

    /// Download and assemble a dataset for a single variable.
    /// Requires the `grib` feature.
    #[cfg(feature = "grib")]
    pub fn build_dataset(&self, var_key: &str, hours: &[u32]) -> Result<GfsDataset> {
        let files = self.download_hours(&[var_key], hours, false)?;
        if files.is_empty() {
            return Err(Error::NoDataForVariable(var_key.to_string()));
        }
        crate::grib_decode::build_single_var_dataset(
            var_key,
            &files,
            self.run_dt,
            &self.date,
            &self.cycle,
        )
    }

    #[cfg(not(feature = "grib"))]
    pub fn build_dataset(&self, _var_key: &str, _hours: &[u32]) -> Result<GfsDataset> {
        Err(Error::FeatureDisabled("grib"))
    }

    /// Download one file per hour for all `var_keys` and build a merged
    /// dataset. Requires the `grib` feature.
    #[cfg(feature = "grib")]
    pub fn build_multi_dataset(&self, var_keys: &[&str], hours: &[u32]) -> Result<GfsDataset> {
        let files = self.download_hours(var_keys, hours, false)?;
        if files.is_empty() {
            return Err(Error::NoFilesAvailable {
                hours: hours.to_vec(),
                date: self.date.clone(),
                cycle: self.cycle.clone(),
            });
        }
        let mut merged: Option<GfsDataset> = None;
        for vk in var_keys {
            if let Ok(ds) = crate::grib_decode::build_single_var_dataset(
                vk,
                &files,
                self.run_dt,
                &self.date,
                &self.cycle,
            ) {
                merged = Some(match merged {
                    Some(m) => m.merge(ds),
                    None => ds,
                });
            }
        }
        merged.ok_or_else(|| Error::other("no variables could be extracted"))
    }

    #[cfg(not(feature = "grib"))]
    pub fn build_multi_dataset(&self, _var_keys: &[&str], _hours: &[u32]) -> Result<GfsDataset> {
        Err(Error::FeatureDisabled("grib"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr() -> GfsDatasetManager {
        let dir = std::env::temp_dir().join(format!("noawclg-test-{}", std::process::id()));
        GfsDatasetManager::new("20260605", "12", dir, None).unwrap()
    }

    #[test]
    fn rejects_bad_cycle() {
        let dir = std::env::temp_dir();
        assert!(GfsDatasetManager::new("20260605", "05", dir, None).is_err());
    }

    #[test]
    fn cache_path_is_deterministic() {
        let m = mgr();
        let p1 = m.cache_path(&["t2m"], 24);
        let p2 = m.cache_path(&["t2m"], 24);
        assert_eq!(p1, p2);
        assert!(p1.to_string_lossy().contains("f024"));
    }

    #[test]
    fn download_hours_rejects_unknown_variable() {
        let m = mgr();
        let err = m.download_hours(&["not-a-var"], &[0], false).unwrap_err();
        assert!(matches!(err, Error::UnknownVariables(_)));
    }

    #[test]
    fn var_params_dedupes_shared_tokens() {
        let m = mgr();
        let params = m.var_params(&["t2m", "d2m"]).unwrap();
        // both are @ 2 m above ground -> lev token should appear once
        assert_eq!(params.matches("lev_2_m_above_ground").count(), 1);
    }
}
