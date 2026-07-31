//! # noawclg
//!
//! Download, analyse and visualise NOAA atmospheric and ocean data.
//!
//! This is a Rust port of the [`noawclg`](https://pypi.org/project/noawclg/)
//! Python package. It gives you a clean Rust API over two major NOAA data
//! streams, **GFS weather forecasts** and **GODAS/ERSST ocean analyses**,
//! with no API key required.
//!
//! ## Feature flags
//!
//! * default: pure logic only (catalogue, coordinate helpers, GRIB2/OPeNDAP
//!   download & caching machinery, ENSO math), no system dependencies.
//! * `grib`: decode downloaded GRIB2 files into [`gfs_dataset::GfsDataset`]s
//!   via the pure-Rust [`gribberish`] crate (pulls in native jpeg2000/png
//!   decoders at build time).
//! * `netcdf-io`: GODAS/ERSST access over OPeNDAP and NetCDF4
//!   save/load, via the [`netcdf`] crate. Requires a system `libnetcdf`
//!   built with DAP support (same runtime requirement the Python library
//!   has via `netCDF4`/`h5netcdf`).
//! * `full`: both of the above.
//!
//! See the crate README for details and a module-by-module comparison with
//! the original Python package.

pub mod catalog;
pub mod coords;
pub mod error;
pub mod gfs_dataset;
#[cfg(feature = "grib")]
pub mod grib_decode;
pub mod http;
pub mod load;
pub mod ocean;
pub mod persistence;
pub mod query;
pub mod view;

pub use catalog::{
    HOURS_10DAYS_3H, HOURS_16DAYS, HOURS_16DAYS_3H, HOURS_5DAYS_1H, MULTILEVEL_VARS, SURFACE_VARS,
    VARIABLES,
};
pub use coords::{auto_date, BoundingBox, Region};
pub use error::{Error, Result};
pub use gfs_dataset::{GfsDataset, GfsDatasetManager, GfsVariable};
pub use load::load;
pub use ocean::{
    classify_enso, enso_summary, enso_summary_from_series, get_currents, get_godas,
    get_nino_anomaly, get_ocean_temp, get_oni, get_salinity, get_ssh, get_sst_series,
    get_thermocline_depth, get_warm_water_volume, nino_anomaly_from_series, oni_from_series,
    open_ersst, open_godas, thermocline_depth_from_field4, warm_water_volume_from_field4,
    EnsoSummaryRow, Field3, Field4, GodasVarMeta, NinoBox, OceanCurrents, OceanField, TimeSeries,
    GODAS_LEVELS, GODAS_VARS, NINO_BOXES,
};
pub use query::{geocode, Coordinate, GetNoaaData, GetNoaaDataOptions};
pub use view::DatasetView;
