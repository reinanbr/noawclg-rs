//! Download, decode (via the system `cdo` tool) and remap DWD ICON global
//! forecast fields into a [`GfsDataset`], the same container type
//! [`crate::gfs_dataset`] uses for GFS — so everything downstream
//! ([`crate::query`], [`crate::view`], [`crate::persistence`]) works
//! identically regardless of which model actually produced the data.
//!
//! ## Why this needs `cdo`
//!
//! ICON global's native grid is unstructured/icosahedral (DWD publishes it
//! as GRIB2 Grid Definition Template 3.101), which [`gribberish`] can't
//! decode (it only implements the lat/lon and Lambert conformal templates —
//! see `grib_decode`'s module docs for the GFS side of that same crate).
//! Rather than hand-roll a GRIB2 unstructured-grid decoder with no way to
//! cross-check correctness against a reference implementation, this shells
//! out to `cdo` (the tool DWD's own documentation recommends for exactly
//! this) to do only the narrow job of "decode this native DWD GRIB2 file
//! into NetCDF, in the same native cell order" — a plain format conversion,
//! not a remap. All the actual regridding math (nearest-neighbor gather
//! from DWD's own published mapping table) is plain, unit-tested Rust in
//! [`crate::icon_grid`], not delegated to `cdo`.
//!
//! Requires the `icon` feature, `cdo` on `PATH` at runtime, and a system
//! `libnetcdf` (via the `netcdf-io` feature it implies). See the README's
//! ICON section.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use ndarray::{ArrayD, IxDyn};

use crate::error::{Error, Result};
use crate::gfs_dataset::{GfsDataset, GfsVariable};
use crate::http::{Fetcher, ReqwestFetcher};
use crate::icon_catalog::{self, IconVarConfig};
use crate::icon_grid::{self, IconRemap};

/// ICON global R3B7's cell count — every single-level field's native-order
/// data array has exactly this many values, regardless of which field.
/// Used to sanity-check `cdo`'s decoded output.
const ICON_GLOBAL_CELLS: usize = 2_949_120;

const BASE_URL: &str = "https://opendata.dwd.de/weather/nwp/icon/grib";

fn valid_cycle(cycle: &str) -> Result<()> {
    if matches!(cycle, "00" | "06" | "12" | "18") {
        Ok(())
    } else {
        Err(Error::InvalidCycle(cycle.to_string()))
    }
}

fn field_url(date: &str, cycle: &str, dwd_name: &str, hour: u32) -> String {
    let stamp = format!("{date}{cycle}");
    let var_upper = dwd_name.to_ascii_uppercase();
    format!(
        "{BASE_URL}/{cycle}/{dwd_name}/icon_global_icosahedral_single-level_{stamp}_{hour:03}_{var_upper}.grib2.bz2"
    )
}

/// Downloads, decodes and remaps ICON global fields for one forecast run.
///
/// Mirrors [`crate::gfs_dataset::GfsDatasetManager`]'s shape (same
/// `date`/`cycle`/`output_dir` cache-directory pattern), but everything
/// past the raw download is ICON-specific.
pub struct IconDatasetManager {
    pub date: String, // YYYYMMDD
    pub cycle: String,
    pub output_dir: PathBuf,
    pub request_timeout: Duration,
    fetcher: Box<dyn Fetcher>,
    run_dt: NaiveDate,
}

impl std::fmt::Debug for IconDatasetManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IconDatasetManager")
            .field("date", &self.date)
            .field("cycle", &self.cycle)
            .field("output_dir", &self.output_dir)
            .finish_non_exhaustive()
    }
}

impl IconDatasetManager {
    /// `date` must be in `YYYYMMDD` format (same convention as
    /// [`crate::gfs_dataset::GfsDatasetManager::new`]).
    pub fn new(date: &str, cycle: &str, output_dir: impl AsRef<Path>) -> Result<Self> {
        Self::with_options(date, cycle, output_dir, Duration::from_secs(60))
    }

    pub fn with_options(
        date: &str,
        cycle: &str,
        output_dir: impl AsRef<Path>,
        request_timeout: Duration,
    ) -> Result<Self> {
        valid_cycle(cycle)?;
        let run_dt = NaiveDate::parse_from_str(date, "%Y%m%d")
            .map_err(|_| Error::InvalidDate(date.to_string()))?;
        let output_dir = output_dir.as_ref().to_path_buf();
        fs::create_dir_all(&output_dir)?;
        let fetcher = ReqwestFetcher::new(request_timeout)?;
        Ok(IconDatasetManager {
            date: date.to_string(),
            cycle: cycle.to_string(),
            output_dir,
            request_timeout,
            fetcher: Box::new(fetcher),
            run_dt,
        })
    }

    fn raw_cache_path(&self, dwd_name: &str, hour: u32) -> PathBuf {
        self.output_dir.join(format!(
            "icon_{}_{}z_{}_f{:03}.grib2",
            self.date, self.cycle, dwd_name, hour
        ))
    }

    fn remapped_cache_path(&self, dwd_name: &str, hour: u32) -> PathBuf {
        self.output_dir.join(format!(
            "icon_{}_{}z_{}_f{:03}.remapped.f64",
            self.date, self.cycle, dwd_name, hour
        ))
    }

    /// Download one (field, hour) GRIB2 file, decompressing the bz2 DWD
    /// serves it as. Returns `Ok(None)` (not an error) when DWD simply
    /// hasn't published this field at this hour — e.g. `VMAX_10M` at hour
    /// 0, or any field past a shorter run's horizon — mirroring how
    /// [`crate::gfs_dataset::GfsDatasetManager::download_hours`] treats
    /// "not there (yet)" as a normal, non-fatal outcome.
    fn download_field_hour(&self, dwd_name: &str, hour: u32) -> Result<Option<PathBuf>> {
        let path = self.raw_cache_path(dwd_name, hour);
        if path.exists() && fs::metadata(&path).map(|m| m.len() > 0).unwrap_or(false) {
            return Ok(Some(path));
        }

        let url = field_url(&self.date, &self.cycle, dwd_name, hour);
        let (status, body) = self.fetcher.get(&url)?;
        if status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            return Err(Error::other(format!(
                "ICON download failed for {dwd_name} f{hour:03}: HTTP {status}"
            )));
        }
        if body.len() < 4 {
            return Ok(None);
        }

        let mut decoder = bzip2::read::BzDecoder::new(body.as_slice());
        let mut grib_bytes = Vec::new();
        std::io::Read::read_to_end(&mut decoder, &mut grib_bytes).map_err(|e| {
            Error::other(format!(
                "bz2 decompression failed for {dwd_name} f{hour:03}: {e}"
            ))
        })?;

        fs::write(&path, &grib_bytes)?;
        Ok(Some(path))
    }

    /// Decode `grib_path` (a plain, single-field, single-hour ICON GRIB2
    /// file, native icosahedral cell order) into that same native order via
    /// `cdo -f nc copy`, i.e. a format conversion with no regridding.
    fn decode_native_values(&self, grib_path: &Path) -> Result<Vec<f64>> {
        which_cdo()?;

        let nc_path = grib_path.with_extension("cdo.nc");
        let output = Command::new("cdo")
            .arg("-s") // quiet
            .arg("-f")
            .arg("nc")
            .arg("copy")
            .arg(grib_path)
            .arg(&nc_path)
            .output()
            .map_err(|e| Error::other(format!("failed to run cdo: {e}")))?;

        if !output.status.success() {
            return Err(Error::other(format!(
                "cdo decode failed for {}: {}",
                grib_path.display(),
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let result = (|| -> Result<Vec<f64>> {
            let file = netcdf::open(&nc_path).map_err(|e| Error::other(e.to_string()))?;
            // The decoded field is the one non-coordinate variable whose
            // flattened length matches ICON global's known cell count —
            // more robust than guessing cdo's chosen variable name, which
            // varies by field (WMO/eccodes shortName lookup).
            for var in file.variables() {
                let Ok(values) = var.get_values::<f64, _>(..) else {
                    continue;
                };
                if values.len() == ICON_GLOBAL_CELLS {
                    return Ok(values);
                }
            }
            Err(Error::other(format!(
                "cdo decoded {} but no variable has ICON global's expected {ICON_GLOBAL_CELLS} cells",
                grib_path.display()
            )))
        })();

        let _ = fs::remove_file(&nc_path);
        result
    }

    /// Full pipeline for one (field, hour): download → decode → remap onto
    /// the regular world grid → cache the result. Returns `Ok(None)` when
    /// the field simply isn't published at this hour.
    fn field_hour_on_world_grid(
        &self,
        dwd_name: &str,
        hour: u32,
        remap: &IconRemap,
    ) -> Result<Option<Vec<f64>>> {
        let cache = self.remapped_cache_path(dwd_name, hour);
        if let Some(cached) = read_f64_cache(&cache, remap.nlat() * remap.nlon())? {
            return Ok(Some(cached));
        }

        let Some(grib_path) = self.download_field_hour(dwd_name, hour)? else {
            return Ok(None);
        };
        let native = self.decode_native_values(&grib_path)?;
        let world = remap.apply(&native);
        write_f64_cache(&cache, &world)?;
        Ok(Some(world))
    }

    /// Download, decode and assemble a [`GfsDataset`] for `canonical_keys`
    /// (e.g. `["t2m", "u10", "v10"]` — see
    /// [`crate::icon_catalog::canonical_keys`] for the full list) across
    /// `hours`. Requires the `icon` feature.
    pub fn build_dataset(&self, canonical_keys: &[&str], hours: &[u32]) -> Result<GfsDataset> {
        if canonical_keys.is_empty() {
            return Err(Error::other("build_dataset: no variables requested"));
        }
        let unknown: Vec<String> = canonical_keys
            .iter()
            .filter(|k| icon_catalog::get(k).is_none())
            .map(|k| k.to_string())
            .collect();
        if !unknown.is_empty() {
            return Err(Error::UnknownIconVariables(unknown));
        }

        let mut sorted_hours = hours.to_vec();
        sorted_hours.sort_unstable();
        sorted_hours.dedup();

        let remap = icon_grid::load_remap(&self.output_dir)?;

        // Fields actually needed off the wire: every requested canonical
        // key's own DWD field, plus u_10m/v_10m whenever "gust" is
        // requested (fallback source for the hours vmax_10m isn't
        // published at — see `icon_catalog`'s `gust` entry).
        let mut needed_fields: Vec<&'static str> = canonical_keys
            .iter()
            .filter_map(|k| icon_catalog::get(k))
            .map(|c| c.dwd_name)
            .collect();
        if canonical_keys.contains(&"gust") {
            needed_fields.push("u_10m");
            needed_fields.push("v_10m");
        }
        needed_fields.sort_unstable();
        needed_fields.dedup();

        // field -> per-hour world-grid values (aligned index-for-index with
        // `sorted_hours`; `None` where that field wasn't published at that
        // hour).
        let mut by_field: HashMap<&str, Vec<Option<Vec<f64>>>> = HashMap::new();
        let total_field_hours = needed_fields.len() * sorted_hours.len();
        let mut done = 0usize;
        for &field in &needed_fields {
            let mut per_hour = Vec::with_capacity(sorted_hours.len());
            for &hour in &sorted_hours {
                done += 1;
                // ICON's cold path is one download + `cdo` decode per
                // (field, hour) — hundreds of round trips with no other
                // signal that it's progressing, not stuck. See
                // `field_hour_on_world_grid`'s docs.
                eprintln!(
                    "[noawclg::icon] fetching {field} f{hour:03} ({done}/{total_field_hours})"
                );
                per_hour.push(self.field_hour_on_world_grid(field, hour, &remap)?);
            }
            by_field.insert(field, per_hour);
        }

        // The master hour axis: hours where the *first requested* field
        // actually came back. Every canonical variable below is built
        // against exactly this axis so the dataset's shared `time`/
        // `forecast_hour` stays valid for every variable in it (see
        // `GfsDataset::merge`'s docs: it trusts each variable's leading
        // dimension already matches the shared axis by construction).
        let anchor_field = icon_catalog::get(canonical_keys[0])
            .expect("validated above")
            .dwd_name;
        let anchor = by_field
            .get(anchor_field)
            .expect("anchor field was fetched above");
        let hour_mask: Vec<bool> = anchor.iter().map(Option::is_some).collect();
        let master_hours: Vec<u32> = sorted_hours
            .iter()
            .zip(&hour_mask)
            .filter(|(_, ok)| **ok)
            .map(|(h, _)| *h)
            .collect();
        if master_hours.is_empty() {
            return Err(Error::NoFilesAvailable {
                hours: hours.to_vec(),
                date: self.date.clone(),
                cycle: self.cycle.clone(),
            });
        }

        let times: Vec<DateTime<Utc>> = master_hours
            .iter()
            .map(|h| {
                Utc.from_utc_datetime(
                    &self
                        .run_dt
                        .and_hms_opt(self.cycle.parse().unwrap_or(0), 0, 0)
                        .unwrap()
                        .checked_add_signed(chrono::Duration::hours(*h as i64))
                        .unwrap(),
                )
            })
            .collect();

        let nlat = remap.nlat();
        let nlon = remap.nlon();
        let mut ds = GfsDataset {
            time: times,
            forecast_hour: master_hours.clone(),
            latitude: remap.latitude.clone(),
            longitude: remap.longitude.clone(),
            level: None,
            ..Default::default()
        };
        ds.attrs.insert("run_date".into(), self.date.clone());
        ds.attrs.insert("run_cycle".into(), self.cycle.clone());
        ds.attrs.insert("model".into(), "icon_global".into());

        // Aligns any per-hour series (already indexed like `sorted_hours`)
        // down onto `master_hours`, filling gaps with the previous
        // master-hour's value (or 0.0 if there is none yet) instead of
        // ever leaving a hole in the shared axis.
        let align = |series: &[Option<Vec<f64>>]| -> Vec<Vec<f64>> {
            let mut out: Vec<Vec<f64>> = Vec::with_capacity(master_hours.len());
            for (v, ok) in series.iter().zip(&hour_mask) {
                if *ok {
                    out.push(v.clone().unwrap_or_else(|| vec![0.0; nlat * nlon]));
                }
            }
            // `series`/`hour_mask` are indexed like `sorted_hours`, and
            // `master_hours` is exactly the subset where `hour_mask` is
            // true, so `out` already has one entry per master hour, in
            // order. Any individual field that's *sometimes* missing at a
            // master hour (anything other than the anchor field itself)
            // still needs its own per-slot fallback — handled by the
            // caller passing already-filled series in.
            out
        };

        for &key in canonical_keys {
            let cfg = icon_catalog::get(key).expect("validated above");
            if cfg.accumulated {
                push_precip_rate(
                    &mut ds,
                    &master_hours,
                    by_field.get("tot_prec"),
                    &hour_mask,
                    nlat,
                    nlon,
                );
                continue;
            }

            let raw_series = by_field.get(cfg.dwd_name).cloned().unwrap_or_default();
            let filled = fill_gaps(key, cfg, &raw_series, &hour_mask, &by_field, nlat, nlon);
            let world_series = align(&filled);
            insert_variable(&mut ds, key, cfg, world_series, nlat, nlon);
        }

        Ok(ds)
    }
}

/// Best-effort gap fill for a field that's `None` at some hour the anchor
/// field succeeded at: `gust` falls back to sustained wind speed; anything
/// else falls back to the previous known value (a "hold" gap-fill), logging
/// once so it's visible without being fatal — DWD's single-level fields are
/// normally all published together, so this path is expected to be rare.
fn fill_gaps(
    canonical: &str,
    cfg: &IconVarConfig,
    series: &[Option<Vec<f64>>],
    hour_mask: &[bool],
    by_field: &HashMap<&str, Vec<Option<Vec<f64>>>>,
    nlat: usize,
    nlon: usize,
) -> Vec<Option<Vec<f64>>> {
    let mut out = Vec::with_capacity(series.len());
    let mut last_good: Option<Vec<f64>> = None;
    for (i, (v, &anchor_ok)) in series.iter().zip(hour_mask).enumerate() {
        if !anchor_ok {
            out.push(None);
            continue;
        }
        if let Some(v) = v {
            last_good = Some(v.clone());
            out.push(Some(v.clone()));
            continue;
        }
        if canonical == "gust" {
            if let (Some(u), Some(v_)) = (
                by_field
                    .get("u_10m")
                    .and_then(|s| s.get(i))
                    .and_then(|x| x.as_ref()),
                by_field
                    .get("v_10m")
                    .and_then(|s| s.get(i))
                    .and_then(|x| x.as_ref()),
            ) {
                let speed: Vec<f64> = u.iter().zip(v_).map(|(a, b)| a.hypot(*b)).collect();
                last_good = Some(speed.clone());
                out.push(Some(speed));
                continue;
            }
        }
        eprintln!(
            "[noawclg::icon] '{}' ({canonical}) missing at a published hour; holding previous value",
            cfg.dwd_name
        );
        out.push(Some(
            last_good.clone().unwrap_or_else(|| vec![0.0; nlat * nlon]),
        ));
    }
    out
}

fn insert_variable(
    ds: &mut GfsDataset,
    key: &str,
    cfg: &IconVarConfig,
    world_series: Vec<Vec<f64>>,
    nlat: usize,
    nlon: usize,
) {
    let ntime = world_series.len();
    let mut flat = Vec::with_capacity(ntime * nlat * nlon);
    for hour_grid in world_series {
        for v in hour_grid {
            flat.push(cfg.converter.map(|f| f(v)).unwrap_or(v));
        }
    }
    let data = ArrayD::from_shape_vec(IxDyn(&[ntime, nlat, nlon]), flat)
        .expect("flat length matches ntime*nlat*nlon by construction");
    ds.var_order.push(key.to_string());
    ds.variables.insert(
        key.to_string(),
        GfsVariable {
            data,
            dims: vec!["time".into(), "latitude".into(), "longitude".into()],
            long_name: cfg.long_name.to_string(),
            units: cfg.units.to_string(),
        },
    );
}

/// `prate` (mm/h) from `tot_prec` (mm accumulated since forecast hour 0):
/// backward-difference each master hour against the previous one, dividing
/// by the elapsed hours. The first master hour has no predecessor, so its
/// rate is the average since forecast start (`tot_prec[h] / h`), or `0` at
/// `h == 0` (nothing has accumulated yet — physically correct, not a
/// missing-data placeholder).
fn push_precip_rate(
    ds: &mut GfsDataset,
    master_hours: &[u32],
    tot_prec: Option<&Vec<Option<Vec<f64>>>>,
    hour_mask: &[bool],
    nlat: usize,
    nlon: usize,
) {
    let Some(tot_prec) = tot_prec else { return };
    let masked: Vec<&Vec<f64>> = tot_prec
        .iter()
        .zip(hour_mask)
        .filter(|(_, ok)| **ok)
        .filter_map(|(v, _)| v.as_ref())
        .collect();
    if masked.len() != master_hours.len() {
        // tot_prec wasn't published at every master hour; rather than
        // fabricate a rate from misaligned accumulations, skip `prate`
        // entirely for this dataset.
        eprintln!("[noawclg::icon] tot_prec missing at some published hours; skipping 'prate'");
        return;
    }

    let mut flat = Vec::with_capacity(master_hours.len() * nlat * nlon);
    let mut prev: Option<(u32, &Vec<f64>)> = None;
    for (&hour, acc) in master_hours.iter().zip(masked.iter()) {
        let rate: Vec<f64> = match prev {
            Some((prev_hour, prev_acc)) => {
                let dt = (hour - prev_hour).max(1) as f64;
                acc.iter()
                    .zip(prev_acc.iter())
                    .map(|(now, before)| ((now - before).max(0.0)) / dt)
                    .collect()
            }
            None if hour > 0 => acc.iter().map(|v| v / hour as f64).collect(),
            None => vec![0.0; nlat * nlon],
        };
        flat.extend(rate);
        prev = Some((hour, acc));
    }

    let cfg = icon_catalog::get("prate").expect("prate is in the catalog");
    let data = ArrayD::from_shape_vec(IxDyn(&[master_hours.len(), nlat, nlon]), flat)
        .expect("flat length matches ntime*nlat*nlon by construction");
    ds.var_order.push("prate".to_string());
    ds.variables.insert(
        "prate".to_string(),
        GfsVariable {
            data,
            dims: vec!["time".into(), "latitude".into(), "longitude".into()],
            long_name: cfg.long_name.to_string(),
            units: cfg.units.to_string(),
        },
    );
}

fn which_cdo() -> Result<()> {
    Command::new("cdo")
        .arg("--version")
        .output()
        .map_err(|_| Error::MissingSystemDependency("cdo"))
        .and_then(|out| {
            if out.status.success() || !out.stderr.is_empty() || !out.stdout.is_empty() {
                // `cdo --version` writes to stderr and exits 0 on every
                // version we've seen; accept any evidence the binary ran.
                Ok(())
            } else {
                Err(Error::MissingSystemDependency("cdo"))
            }
        })
}

fn write_f64_cache(path: &Path, values: &[f64]) -> Result<()> {
    let mut bytes = Vec::with_capacity(values.len() * 8);
    for v in values {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn read_f64_cache(path: &Path, expected_len: usize) -> Result<Option<Vec<f64>>> {
    if !path.exists() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    if bytes.len() != expected_len * 8 {
        let _ = fs::remove_file(path);
        return Ok(None);
    }
    Ok(Some(
        bytes
            .chunks_exact(8)
            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_url_matches_dwd_open_data_layout() {
        let url = field_url("20260801", "00", "t_2m", 0);
        assert_eq!(
            url,
            "https://opendata.dwd.de/weather/nwp/icon/grib/00/t_2m/icon_global_icosahedral_single-level_2026080100_000_T_2M.grib2.bz2"
        );
    }

    #[test]
    fn field_url_uppercases_only_the_variable_token() {
        let url = field_url("20260801", "12", "vmax_10m", 1);
        assert!(url.contains("/12/vmax_10m/"));
        assert!(url.ends_with("_001_VMAX_10M.grib2.bz2"));
    }

    #[test]
    fn rejects_bad_cycle() {
        let dir = std::env::temp_dir();
        assert!(IconDatasetManager::new("20260801", "05", dir).is_err());
    }

    #[test]
    fn f64_cache_round_trips() {
        let dir =
            std::env::temp_dir().join(format!("noawclg-icon-cache-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.f64");
        let values = vec![1.5, -2.25, 3.0, 0.0];
        write_f64_cache(&path, &values).unwrap();
        let read_back = read_f64_cache(&path, values.len()).unwrap().unwrap();
        assert_eq!(read_back, values);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn f64_cache_rejects_wrong_length_and_reports_absent() {
        let dir =
            std::env::temp_dir().join(format!("noawclg-icon-cache-test2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sample.f64");
        write_f64_cache(&path, &[1.0, 2.0]).unwrap();
        assert!(read_f64_cache(&path, 5).unwrap().is_none());
        assert!(
            !path.exists(),
            "corrupt/mismatched cache entry should be evicted"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
