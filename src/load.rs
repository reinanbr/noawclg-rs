//! Convenience wrapper: load GFS data directly as a [`GfsDataset`].
//!
//! Direct port of `noawclg/load.py`.

use crate::coords::Region;
use crate::error::Result;
use crate::gfs_dataset::GfsDataset;
use crate::query::{GetNoaaData, GetNoaaDataOptions};

/// Load NOAA GFS data and return the underlying [`GfsDataset`] directly.
///
/// `date` is in `'DD/MM/YYYY'` format. Requires the `grib` feature to
/// actually decode GRIB2 data (see [`crate::gfs_dataset::GfsDatasetManager`]).
///
/// Mirrors `noawclg.load.load`.
pub fn load(
    date: &str,
    cycle: &str,
    keys: Vec<String>,
    hours: Vec<u32>,
    region: Option<Region>,
) -> Result<GfsDataset> {
    let noaa = GetNoaaData::new(
        date,
        cycle,
        keys,
        hours,
        GetNoaaDataOptions {
            region,
            ..Default::default()
        },
    )?;
    Ok(noaa.into_dataset())
}
