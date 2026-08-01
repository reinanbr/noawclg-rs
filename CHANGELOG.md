# Changelog

All notable changes to **noawclg** (Rust) are documented here.

This project follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/)
and adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

This crate is a Rust port of the Python
[`noawclg`](https://github.com/reinanbr/noawclg) package. Version numbers
track the Python package's version they were ported from, not a separate
sequence.

---

## [2.4.0] - 2026-08-01

### Added

DWD ICON global forecast support — a Rust-only addition with no
counterpart in the upstream Python `noawclg` package (everything through
2.3.1 was a straight port; this is the first feature that isn't).

- `icon_dataset::IconDatasetManager`: downloads, decodes and assembles
  ICON global forecast fields into a `gfs_dataset::GfsDataset` — the same
  container type GFS uses, with the same canonical variable keys
  (`t2m`, `d2m`, `r2`, `u10`, `v10`, `gust`, `prmsl`, `prate`, `tcc`,
  `cape`), so callers don't need model-specific code to use either.
  - ICON global's native grid is unstructured/icosahedral, which
    `gribberish` can't decode (only lat/lon and Lambert conformal grid
    templates are implemented). Rather than hand-roll an unstructured
    -grid GRIB2 decoder with no reference to validate against, this
    shells out to the system `cdo` tool for the narrow job of decoding
    raw GRIB2 into NetCDF (a format conversion, not a remap), then
    applies DWD's own published nearest-neighbor mapping in plain,
    unit-tested Rust (`icon_grid`) to land the result on a regular
    0.25° world lat/lon grid.
  - `prate` is derived from ICON's `TOT_PREC` (accumulated since
    forecast hour 0) by backward-differencing consecutive requested
    hours, since ICON doesn't publish an instantaneous rate field the
    way GFS does.
  - `gust` falls back to sustained wind speed (`hypot(u10, v10)`) for
    hours where ICON's `VMAX_10M` isn't published (notably forecast
    hour 0, which has no preceding averaging interval yet), so the
    variable's hour axis never has a gap.
  - GFS's categorical precipitation-type flags (`crain`/`csnow`/
    `cfrzr`/`cicep`) have no honest ICON equivalent in the open-data
    single-level feed and are left unmapped rather than approximated.
- `icon_catalog`: the ICON→canonical variable table and ICON global's
  own forecast-hour cadence (`default_hours`: 0-78 h @ 1 h, then
  81-180 h @ 3 h).
- `icon_grid::load_remap`: downloads (once, cached indefinitely) and
  parses DWD's published `ICON_GLOBAL2WORLD_025_EASY` SCRIP remap-weight
  archive into a gather table.
- New `icon` feature flag (implies `netcdf-io`; also requires `cdo` on
  `PATH` at runtime, checked with a clear `Error::MissingSystemDependency`
  rather than a build-time requirement). `full` now includes it.

### Testing
- Unit tests for the ICON catalogue, URL building, SCRIP grid-file
  parsing, and the remap gather/averaging logic — all offline, no `cdo`
  or network required.
- `tests/icon_integration_tests.rs`: opt-in (`cargo test --features icon
  -- --ignored`) live tests against the real DWD open-data feed and a
  real `cdo` invocation, verified during development against the live
  2026-08-01 00Z run — including a physical plausibility check (São
  Paulo winter 2 m temperature in range) and a check that derived
  `prate` is never negative, not just "the code didn't panic."

## [2.3.1] - 2026-07-31

### Changed
- Documentation cleanup across the README, CHANGELOG, and all doc
  comments: em dashes replaced with plain punctuation throughout.
- The Nominatim geocoding client's `User-Agent` header is now derived
  from `CARGO_PKG_VERSION` at compile time instead of a hand-maintained
  literal, so it can't drift from the crate version again.

No functional changes.

## [2.3.0] - 2026-07-31

### Added

Initial Rust release: a module-for-module port of `noawclg` 2.3.0.

#### GFS weather forecasts
- `catalog`: the full 47-entry `VARIABLES` catalogue, `SURFACE_VARS` /
  `MULTILEVEL_VARS`, and the `HOURS_5DAYS_1H` / `HOURS_10DAYS_3H` /
  `HOURS_16DAYS_3H` / `HOURS_16DAYS` forecast-hour sequences.
- `coords`: `BoundingBox`, `auto_date`, `find_dim`, `normalize_lon`,
  `parse_date`.
- `http`: the NOMADS grib-filter URL builder and an injectable
  `Fetcher` trait (real `ReqwestFetcher` plus in-memory `FakeFetcher` for
  tests) standing in for the Python tests' `unittest.mock` patches.
- `gfs_dataset::GfsDatasetManager`: cached, retrying GRIB2 download
  (`download_hours`) always available. `build_dataset` and
  `build_multi_dataset` decode via the pure-Rust `gribberish` crate,
  gated behind the `grib` feature.
- `query::GetNoaaData`: point/place queries (`get_data_from_point`,
  `get_data_from_place` via Nominatim geocoding, `get_time_series`),
  `view::DatasetView`, and the `load()` one-liner.
- `persistence`: a self-contained Zarr v2 writer/reader (always
  available, no external dependency) and NetCDF4 save/load gated behind
  the `netcdf-io` feature.

#### Ocean data & ENSO diagnostics
- `ocean`: `open_godas` / `get_godas` and the typed wrappers
  (`get_ocean_temp`, `get_salinity`, `get_currents`, `get_ssh`),
  `open_ersst`, `get_sst_series`, `get_nino_anomaly`, `get_oni`,
  `classify_enso`, `get_thermocline_depth` (D20), `get_warm_water_volume`
  (WWV), and `enso_summary`. All pure index math (masking, unit
  conversion, region/depth selection, anomaly/ONI/classification) is
  always compiled and unit-tested independent of the `netcdf-io` feature
  that OPeNDAP access itself requires.

### Feature flags
- *(default)*: pure logic only, no system dependencies.
- `grib`: real GRIB2 decoding via [`gribberish`](https://crates.io/crates/gribberish).
- `netcdf-io`: GODAS/ERSST/NetCDF4 via the [`netcdf`](https://crates.io/crates/netcdf)
  crate (requires system `libnetcdf` built with DAP support).
- `full`: both of the above.

### Testing
- 139 tests with default features (147 with `--features grib`): unit
  tests with access to crate internals, plus a `tests/` integration
  suite that exercises every README example and every
  `test_gfs_dataset.py` / `test_main.py` / `test_ocean.py` case from the
  Python test suite through the public API only.
- `tests/integration_live_tests.rs`: opt-in (`cargo test -- --ignored`)
  real NOMADS network tests, always resolving dates dynamically via
  `auto_date` rather than a hardcoded date, matching NOMADS's rolling
  few-day retention window.

### Known limitations
- No plotting. `plots.py` lives outside the Python `noawclg` package
  itself, so it's out of scope for this port.
- `grib_decode.rs` is written against `gribberish`'s documented API but
  has not been exercised against a real downloaded GFS file in the
  environment this was built in.
