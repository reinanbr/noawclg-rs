//! DWD's published nearest-neighbor remap table from the ICON global
//! icosahedral grid onto a regular 0.25° world lat/lon grid.
//!
//! DWD publishes this as a ready-to-use CDO "EASY" remap package
//! (`ICON_GLOBAL2WORLD_025_EASY.tar.bz2`, ~44 MB) at
//! <https://opendata.dwd.de/weather/lib/cdo/> — a SCRIP-format sparse remap
//! matrix (`weights_icogl2world_025.nc`) plus a CDO grid description
//! (`target_grid_world_025.txt`). Empirically (verified against the
//! published file) its `map_method` is "Nearest neighbor" with exactly one
//! link per destination cell and every remap weight equal to `1.0`, i.e.
//! it's a pure gather table — but this reads the general SCRIP sparse
//! matrix form (`dst_address`/`src_address`/`remap_matrix`) rather than
//! assuming that shape, so it keeps working if DWD ever republishes a
//! bilinear version of the same file.
//!
//! Only [`load_remap`] is public; everything else here is implementation
//! detail of turning that archive into a `[dst_index] -> [(src_index,
//! weight), ...]` gather table.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::{Error, Result};
use crate::http::{Fetcher, ReqwestFetcher};

const EASY_URL: &str = "https://opendata.dwd.de/weather/lib/cdo/ICON_GLOBAL2WORLD_025_EASY.tar.bz2";
const ARCHIVE_DIR: &str = "ICON_GLOBAL2WORLD_025_EASY";
const WEIGHTS_FILE: &str = "weights_icogl2world_025.nc";
const TARGET_GRID_FILE: &str = "target_grid_world_025.txt";

/// The regular target grid, plus, per target cell, which source ICON
/// cell(s) contribute to it and with what weight.
pub struct IconRemap {
    /// Ascending, degrees. 721 points at 0.25° (-90..=90, inclusive of both
    /// poles), matching `target_grid_world_025.txt`.
    pub latitude: Vec<f64>,
    /// Ascending, degrees [0, 360). 1440 points at 0.25°.
    pub longitude: Vec<f64>,
    /// Row-major `[lat][lon]` flattened, matching `latitude`/`longitude`
    /// above: `gather[iy * longitude.len() + ix]`.
    gather: Vec<Vec<(u32, f64)>>,
}

impl IconRemap {
    /// Apply this remap to one ICON field's native-order cell values
    /// (length must match the source ICON grid's cell count — indices past
    /// the end are treated as missing and contribute `0`, so a length
    /// mismatch degrades rather than panics; callers should still treat
    /// that as a decode bug).
    ///
    /// Returns a flat row-major `[lat][lon]` array, same shape as
    /// `latitude.len() * longitude.len()`.
    pub fn apply(&self, source_values: &[f64]) -> Vec<f64> {
        self.gather
            .iter()
            .map(|links| {
                links.iter().fold(0.0, |acc, &(idx, w)| {
                    acc + w * source_values.get(idx as usize).copied().unwrap_or(0.0)
                })
            })
            .collect()
    }

    pub fn nlat(&self) -> usize {
        self.latitude.len()
    }

    pub fn nlon(&self) -> usize {
        self.longitude.len()
    }
}

/// Parse the CDO grid-description text file (`gridtype = lonlat`,
/// `xsize = 1440`, ...) for the six fields that fully determine a regular
/// lat/lon grid.
fn parse_target_grid(text: &str) -> Result<(usize, usize, f64, f64, f64, f64)> {
    let mut kv = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            kv.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let get = |k: &str| -> Result<&String> {
        kv.get(k)
            .ok_or_else(|| Error::other(format!("target grid file missing '{k}'")))
    };
    let parse_f = |k: &str| -> Result<f64> {
        get(k)?
            .parse()
            .map_err(|_| Error::other(format!("target grid file: bad value for '{k}'")))
    };
    let parse_u = |k: &str| -> Result<usize> {
        get(k)?
            .parse()
            .map_err(|_| Error::other(format!("target grid file: bad value for '{k}'")))
    };
    Ok((
        parse_u("xsize")?,
        parse_u("ysize")?,
        parse_f("xfirst")?,
        parse_f("xinc")?,
        parse_f("yfirst")?,
        parse_f("yinc")?,
    ))
}

/// Download (if not already cached under `cache_dir`) and parse DWD's
/// icosahedral → 0.25° world grid remap table.
pub fn load_remap(cache_dir: impl AsRef<Path>) -> Result<IconRemap> {
    let dir = ensure_downloaded(cache_dir.as_ref())?;

    let grid_text = fs::read_to_string(dir.join(TARGET_GRID_FILE))?;
    let (xsize, ysize, xfirst, xinc, yfirst, yinc) = parse_target_grid(&grid_text)?;

    let longitude: Vec<f64> = (0..xsize).map(|i| xfirst + xinc * i as f64).collect();
    let latitude: Vec<f64> = (0..ysize).map(|i| yfirst + yinc * i as f64).collect();
    let dst_len = xsize * ysize;

    let file = netcdf::open(dir.join(WEIGHTS_FILE)).map_err(|e| Error::other(e.to_string()))?;
    let read_f64 = |name: &str| -> Result<Vec<f64>> {
        file.variable(name)
            .ok_or_else(|| Error::other(format!("weights file missing '{name}'")))?
            .get_values::<f64, _>(..)
            .map_err(|e| Error::other(e.to_string()))
    };
    let read_i32 = |name: &str| -> Result<Vec<i32>> {
        file.variable(name)
            .ok_or_else(|| Error::other(format!("weights file missing '{name}'")))?
            .get_values::<i32, _>(..)
            .map_err(|e| Error::other(e.to_string()))
    };

    let dst_address = read_i32("dst_address")?;
    let src_address = read_i32("src_address")?;
    // Shape `(num_links, num_wgts)`, flattened; for this NN table
    // `num_wgts` is 1, but the general form is read anyway (see module docs).
    let remap_matrix = read_f64("remap_matrix")?;

    if dst_address.len() != src_address.len() {
        return Err(Error::other(
            "ICON remap weights file: dst_address/src_address length mismatch",
        ));
    }
    let num_links = dst_address.len();
    let num_wgts = remap_matrix.len().checked_div(num_links).unwrap_or(1);

    let mut gather: Vec<Vec<(u32, f64)>> = vec![Vec::new(); dst_len];
    for link in 0..num_links {
        let dst1 = dst_address[link];
        let src1 = src_address[link];
        if dst1 < 1 || src1 < 1 {
            continue;
        }
        let dst_idx = (dst1 - 1) as usize;
        let src_idx = (src1 - 1) as u32;
        let weight = remap_matrix.get(link * num_wgts).copied().unwrap_or(1.0);
        if let Some(bucket) = gather.get_mut(dst_idx) {
            bucket.push((src_idx, weight));
        }
    }

    Ok(IconRemap {
        latitude,
        longitude,
        gather,
    })
}

/// Ensure the EASY archive is downloaded + extracted under `cache_dir`,
/// returning the directory containing its two files. Cached indefinitely —
/// the ICON grid topology this describes doesn't change between forecast
/// runs, only between ICON model-version upgrades (rare, and DWD would
/// publish a new EASY archive for that; this crate would need a version
/// bump to point at it, same as any other upstream format change).
fn ensure_downloaded(cache_dir: &Path) -> Result<PathBuf> {
    let dir = cache_dir.join(ARCHIVE_DIR);
    if dir.join(WEIGHTS_FILE).exists() && dir.join(TARGET_GRID_FILE).exists() {
        return Ok(dir);
    }
    fs::create_dir_all(cache_dir)?;

    let fetcher = ReqwestFetcher::new(Duration::from_secs(180))?;
    let (status, body) = fetcher.get(EASY_URL)?;
    if !(200..300).contains(&status) {
        return Err(Error::other(format!(
            "failed to download ICON remap weights from {EASY_URL}: HTTP {status}"
        )));
    }

    extract_tar_bz2(&body, cache_dir)?;
    if !dir.join(WEIGHTS_FILE).exists() {
        return Err(Error::other(
            "ICON remap archive downloaded and extracted, but the expected weights file is missing",
        ));
    }
    Ok(dir)
}

/// Decompress `bytes` as bz2, then unpack the resulting tar into `dest`.
fn extract_tar_bz2(bytes: &[u8], dest: &Path) -> Result<()> {
    use std::io::Read;

    let mut decoder = bzip2::read::BzDecoder::new(bytes);
    let mut tar_bytes = Vec::new();
    decoder
        .read_to_end(&mut tar_bytes)
        .map_err(|e| Error::other(format!("bz2 decompression failed: {e}")))?;

    let mut archive = tar::Archive::new(tar_bytes.as_slice());
    archive
        .unpack(dest)
        .map_err(|e| Error::other(format!("tar extraction failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_target_grid_reads_dwd_world_025_file() {
        let text = "\
# CDO grid description file for global regular grid of ICON.
gridtype = lonlat
xsize    = 1440
ysize    =  721
xfirst   =   0.0
xinc     =   0.25
yfirst   = -90.0
yinc     =   0.25
";
        let (xsize, ysize, xfirst, xinc, yfirst, yinc) = parse_target_grid(text).unwrap();
        assert_eq!(xsize, 1440);
        assert_eq!(ysize, 721);
        assert_eq!(xfirst, 0.0);
        assert_eq!(xinc, 0.25);
        assert_eq!(yfirst, -90.0);
        assert_eq!(yinc, 0.25);
    }

    #[test]
    fn parse_target_grid_errors_on_missing_key() {
        let err = parse_target_grid("gridtype = lonlat\nxsize = 10\n").unwrap_err();
        assert!(err.to_string().contains("ysize"));
    }

    #[test]
    fn icon_remap_apply_is_a_pure_gather_for_nn_weights() {
        // 2x2 destination grid, each cell nearest-neighbor-mapped to a
        // distinct source cell (mirrors the real file's map_method=NN,
        // weight=1.0 shape) — this exercises `apply` the same way the real
        // 1,038,240-cell table would, just at toy scale.
        let remap = IconRemap {
            latitude: vec![-10.0, 10.0],
            longitude: vec![0.0, 90.0],
            gather: vec![
                vec![(2, 1.0)],
                vec![(0, 1.0)],
                vec![(3, 1.0)],
                vec![(1, 1.0)],
            ],
        };
        let source = vec![10.0, 20.0, 30.0, 40.0];
        let out = remap.apply(&source);
        assert_eq!(out, vec![30.0, 10.0, 40.0, 20.0]);
    }

    #[test]
    fn icon_remap_apply_averages_multi_link_destinations() {
        // General SCRIP form: a destination fed by two source cells with
        // 0.5/0.5 weights (not how the real NN file is shaped, but the
        // gather logic must still handle it correctly).
        let remap = IconRemap {
            latitude: vec![0.0],
            longitude: vec![0.0],
            gather: vec![vec![(0, 0.5), (1, 0.5)]],
        };
        assert_eq!(remap.apply(&[10.0, 20.0]), vec![15.0]);
    }

    #[test]
    fn icon_remap_apply_treats_out_of_range_source_index_as_zero() {
        let remap = IconRemap {
            latitude: vec![0.0],
            longitude: vec![0.0],
            gather: vec![vec![(99, 1.0)]],
        };
        assert_eq!(remap.apply(&[1.0, 2.0]), vec![0.0]);
    }
}
