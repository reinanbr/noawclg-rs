# noawclg (Rust)

[![Crates.io](https://img.shields.io/crates/v/noawclg.svg)](https://crates.io/crates/noawclg)
[![docs.rs](https://img.shields.io/docsrs/noawclg)](https://docs.rs/noawclg)
[![Crates.io Downloads](https://img.shields.io/crates/d/noawclg.svg)](https://crates.io/crates/noawclg)
[![CI](https://github.com/reinanbr/noawclg-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/reinanbr/noawclg-rs/actions/workflows/ci.yml)
[![License: GPLv3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![MSRV](https://img.shields.io/badge/MSRV-1.80-orange.svg)](Cargo.toml)

Rust port of the [`noawclg`](https://pypi.org/project/noawclg/) Python
package: download, analyse and visualise NOAA atmospheric (GFS), DWD ICON
global, and ocean (GODAS / ERSST) data, no API key required.

This crate mirrors the Python package module-for-module for everything
through GFS/ocean support. ICON global support (`icon_catalog`,
`icon_dataset`, `icon_grid`) is a Rust-only addition with no Python
counterpart — see [Module map](#module-map--python-equivalent) below for
the exact correspondence on the ported side.

## Feature flags

| Feature | Adds | System requirement |
|---|---|---|
| *(default)* | catalogue, coordinate helpers, GFS GRIB2 download & caching, Zarr save/load, all ENSO/ocean math | none |
| `grib` | Decodes downloaded GFS GRIB2 files into datasets, via the pure-Rust [`gribberish`](https://crates.io/crates/gribberish) crate | none (builds a vendored JPEG2000/PNG decoder) |
| `netcdf-io` | GODAS/ERSST access over OPeNDAP + NetCDF4 save/load, via the [`netcdf`](https://crates.io/crates/netcdf) crate | system `libnetcdf` built with DAP support (e.g. `apt install libnetcdf-dev libhdf5-dev`), discovered via `pkg-config`/`NETCDF_DIR`. `netcdf-sys` has no automatic vendored fallback — it only builds from source if the `static` feature is explicitly enabled on the `netcdf`/`netcdf-sys` crates, which this crate doesn't do. |
| `icon` | Downloads and decodes DWD ICON global forecasts into the same dataset shape GFS uses, via [`icon_dataset`]. Implies `netcdf-io`. | Same `libnetcdf` requirement as `netcdf-io`, plus `cdo` on `PATH` **at runtime** (e.g. `apt install cdo`) — checked with a clear error, not a build-time requirement |
| `full` | all of the above | all of the above |

`cargo build`/`cargo test` with **no** features succeeds anywhere and
exercises 100% of the pure logic (unit tests run without network or system
libs). `netcdf-io`/`icon` need `libnetcdf-dev` installed to even compile
(see above — this bit us during development: a machine that happened to
already have it installed masked the requirement until CI, which doesn't,
caught it); `icon`'s live/integration tests additionally need `cdo`
installed and network access to `opendata.dwd.de`.

```bash
cargo build                        # pure logic only
cargo build --features grib        # + GFS GRIB2 decoding
cargo build --features netcdf-io   # + GODAS/ERSST/NetCDF (needs libnetcdf-dev)
cargo build --features icon        # + DWD ICON global (needs libnetcdf-dev + `cdo` at runtime)
cargo build --features full        # everything
```

[`icon_dataset`]: https://docs.rs/noawclg/latest/noawclg/icon_dataset/index.html

## Data freshness: always resolve dates dynamically

**NOMADS only serves a rolling window of recent GFS runs.** Treat anything
older than 3 days back as unavailable. Every example below resolves its
date from "now" via [`auto_date`], never a hardcoded date. If you write your
own code against this crate, do the same: a date more than a few days old
will 404. This is exactly why [`auto_date`]'s `lag_days` parameter exists:
`auto_date(1)` targets yesterday's run, which is reliably published by the
time you'd read this.

[`auto_date`]: https://docs.rs/noawclg/latest/noawclg/fn.auto_date.html

## Examples

All examples below are ported directly from the
[Python README](https://github.com/reinanbr/noawclg#readme) and exercised
in `tests/readme_examples_tests.rs` (offline, against synthetic/in-memory
data standing in for a real download or OPeNDAP fetch; see that file's
module doc for exactly which Python example each test corresponds to).
`tests/integration_live_tests.rs` additionally hits the *real* NOMADS
service for the download step (run with `cargo test -- --ignored`).

### GFS weather forecasts

#### `auto_date`: pick the latest available GFS cycle

```rust
use noawclg::auto_date;

let (date, cycle) = auto_date(1);
// date  -> "30/07/2026"  (DD/MM/YYYY, always this format)
// cycle -> "12"          (00 / 06 / 12 / 18)
```

#### `load`: quick download as a `GfsDataset`

Requires the `grib` feature to actually decode the downloaded GRIB2 files.
There are no `lat=`/`lon=` parameters; to query a single point, use
[`GetNoaaData::get_data_from_point`](#get_noaa_data--query-by-coordinates-or-place-name)
instead.

```rust
use noawclg::{auto_date, load, Region};

let (date, cycle) = auto_date(1);

let ds = load(
    &date,
    &cycle,
    vec!["t2m".into(), "u10".into(), "v10".into(), "prmsl".into(), "prate".into()],
    (0..=120).step_by(3).collect(),
    Some(Region { toplat: 5.0, bottomlat: -15.0, leftlon: -50.0, rightlon: -30.0 }),
)?;

println!("{} variables, {} time steps", ds.var_order.len(), ds.time.len());
```

#### Pre-defined hour sequences

```rust
use noawclg::{load, HOURS_5DAYS_1H, HOURS_10DAYS_3H, HOURS_16DAYS_3H};

let ds = load(&date, &cycle, vec!["t2m".into()], HOURS_5DAYS_1H.clone(), None)?;
```

| Constant | Range | Step |
|---|---|---|
| `HOURS_5DAYS_1H` | 0–120 h | 1 h |
| `HOURS_10DAYS_3H` | 0–240 h | 3 h |
| `HOURS_16DAYS_3H` | 0–384 h | 3 h |
| `HOURS_16DAYS` | 0–120 h @ 6 h + 123–384 h @ 3 h | mixed |

#### `GfsDatasetManager`: full control over download and storage

`GfsDatasetManager` takes `date` in **`YYYYMMDD`** format (`auto_date`
returns `DD/MM/YYYY`; convert with `noawclg::coords::parse_date`).

```rust
use noawclg::{coords::parse_date, persistence, GfsDatasetManager, Region};

let (date, cycle) = noawclg::auto_date(1);
let mgr = GfsDatasetManager::new(
    &parse_date(&date)?,
    &cycle,
    "gfs_cache/",
    Some(Region { toplat: 10.0, bottomlat: -20.0, leftlon: -55.0, rightlon: -25.0 }),
)?;

let hours: Vec<u32> = (0..=48).step_by(3).collect();

// One variable -> build_dataset
let ds_t = mgr.build_dataset("t2m", &hours)?;              // needs `grib`

// Multiple variables at once -> build_multi_dataset
let ds = mgr.build_multi_dataset(&["t2m", "u10", "v10", "prmsl"], &hours)?; // needs `grib`

// Persist and reload
persistence::save_zarr(&ds, "forecast.zarr")?;
let ds2 = persistence::load_zarr("forecast.zarr")?;

persistence::save_netcdf(&ds, "forecast.nc")?;              // needs `netcdf-io`
let ds3 = persistence::load_netcdf("forecast.nc")?;          // needs `netcdf-io`
```

#### `GetNoaaData`: query by coordinates or place name

Date format is `DD/MM/YYYY`, same as `auto_date`'s output.

```rust
use noawclg::{GetNoaaData, GetNoaaDataOptions, Region};

let (date, cycle) = noawclg::auto_date(1);
let gfs = GetNoaaData::new(
    &date,
    &cycle,
    vec!["t2m".into(), "prmsl".into(), "prate".into(), "u10".into(), "v10".into()],
    (0..=72).step_by(3).collect(),
    GetNoaaDataOptions {
        region: Some(Region { toplat: 5.0, bottomlat: -15.0, leftlon: -50.0, rightlon: -30.0 }),
        ..Default::default()
    },
)?; // needs `grib`

// Query by coordinates -> DatasetView
let view = gfs.get_data_from_point((-3.7, -38.5), None)?;
let table = view.to_table(); // Vec<BTreeMap<String, f64>>, one row per forecast hour

// Query by place name (geocoded automatically via Nominatim)
let view2 = gfs.get_data_from_place("Recife PE", None)?;

// Complete time series for one variable at a grid point
let t2m_series = gfs.get_time_series((-3.7, -38.5), Some("t2m"))?;

// List all loaded variables
println!("{:?}", gfs.get_keys());

// Access the raw dataset
let raw = gfs.dataset();
```

### ICON global forecasts

Needs the `icon` feature and `cdo` on `PATH` (`apt install cdo`, or your
platform's equivalent). Same `GfsDataset` shape as GFS, same canonical
variable keys — this really is meant to be a drop-in alternative model,
not a separate API.

```rust
use noawclg::{GetNoaaData, IconDatasetManager};

let (date, cycle) = noawclg::auto_date(1);
let date_ymd = date.replace('/', ""); // IconDatasetManager wants YYYYMMDD, same as GfsDatasetManager::new

let mgr = IconDatasetManager::new(&date_ymd, &cycle, "./icon_output")?;
let ds = mgr.build_dataset(
    &["t2m", "u10", "v10", "gust", "prmsl", "prate", "tcc", "cape"],
    &noawclg::icon_catalog::default_hours(), // 0-78h @ 1h, 81-180h @ 3h
)?;

// Same `GetNoaaData::from_dataset` + `DatasetView` API as the GFS example
// above works unchanged, since `ds` is a plain `GfsDataset`:
let noaa = GetNoaaData::from_dataset(date_ymd, cycle, vec!["t2m".into()], ds.forecast_hour.clone(), ds);
let view = noaa.get_data_from_point((-23.55, -46.63), None)?; // São Paulo
```

Why this needs `cdo`, what's derived vs. directly mapped (`prate` from
`TOT_PREC`, `gust`'s hour-0 fallback), and which GFS fields have no ICON
equivalent (the categorical precipitation-type flags) are documented on
[`icon_dataset`] and [`icon_catalog`].

[`icon_catalog`]: https://docs.rs/noawclg/latest/noawclg/icon_catalog/index.html

### Mathematical analysis examples

All examples below assume a dataset loaded as in the `load` example, plus a
single grid point selected: the same shape of computation the Python
README does with `numpy`/`pandas`/`scipy`, translated to plain Rust +
`ndarray` (this crate's numeric dependency, no extra math crate required).
A few Python examples that lean on `scipy.signal`/`scipy.ndimage`
(`find_peaks`, `gaussian_filter`, FFT dominant-period detection) are **not**
ported. They'd need `rustfft`/an image-filtering crate, which is outside
`noawclg`'s own scope in either language (the Python versions are usage
demos, not part of the `noawclg` package either).

#### Temperature: heat index, anomaly, trend

```rust
// Heat index (Rothfusz equation, °C in -> °C out)
fn heat_index(t: f64, rh: f64) -> f64 {
    -8.78469475556
        + 1.61139411 * t
        + 2.33854883889 * rh
        - 0.14611605 * t * rh
        - 0.012308094 * t * t
        - 0.0164248277778 * rh * rh
        + 0.002211732 * t * t * rh
        + 0.00072546 * t * rh * rh
        - 0.000003582 * t * t * rh * rh
}

// Anomaly relative to the 0-h (analysis) step
let t2m_anom: Vec<f64> = t2m.iter().map(|v| v - t2m[0]).collect();

// Linear trend across the forecast window (simple least squares)
fn linregress(x: &[f64], y: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let cov: f64 = x.iter().zip(y).map(|(xi, yi)| (xi - mx) * (yi - my)).sum();
    let var: f64 = x.iter().map(|xi| (xi - mx).powi(2)).sum();
    let slope = cov / var;
    (slope, my - slope * mx)
}
let hours_f: Vec<f64> = hours.iter().map(|h| *h as f64).collect();
let (slope, _intercept) = linregress(&hours_f, &t2m);
println!("Warming rate: {slope:.3} °C/h");
```

#### Dew-point depression and relative humidity check

```rust
// Dew-point depression (dry-bulb minus dew-point, °C)
let depression: Vec<f64> = t2m.iter().zip(&d2m).map(|(t, td)| t - td).collect();

// Magnus formula: recompute RH from T and Td to cross-check
const A: f64 = 17.625;
const B: f64 = 243.04;
let rh_check: Vec<f64> = t2m.iter().zip(&d2m).map(|(t, td)| {
    100.0 * (A * td / (B + td)).exp() / (A * t / (B + t)).exp()
}).collect();
```

#### Wind: speed, direction, stress, gusts

```rust
// Scalar wind speed and meteorological direction (from, 0°=N)
let wspd: Vec<f64> = u10.iter().zip(&v10).map(|(u, v)| u.hypot(*v)).collect();
let wdir: Vec<f64> = u10.iter().zip(&v10).map(|(u, v)| {
    (270.0 - v.atan2(*u).to_degrees()).rem_euclid(360.0)
}).collect();

// Wind stress (bulk formula, air density rho ~ 1.225 kg/m3, Cd ~ 1.3e-3)
const RHO: f64 = 1.225;
const CD: f64 = 1.3e-3;
let tau_x: Vec<f64> = wspd.iter().zip(&u10).map(|(s, u)| RHO * CD * s * u).collect();
let tau_y: Vec<f64> = wspd.iter().zip(&v10).map(|(s, v)| RHO * CD * s * v).collect();

// Normalised gust factor (gust / sustained)
let gust_factor: Vec<f64> = gust.iter().zip(&wspd)
    .map(|(g, s)| if *s > 0.0 { g / s } else { f64::NAN })
    .collect();

// Beaufort scale
fn beaufort(speed: f64) -> u8 {
    const EDGES: [f64; 12] = [0.3, 1.6, 3.4, 5.5, 8.0, 10.8, 13.9, 17.2, 20.8, 24.5, 28.5, 32.7];
    EDGES.iter().filter(|&&e| speed >= e).count() as u8
}
```

#### Pressure: gradient and tendency

```rust
use ndarray::Array3; // prmsl: (time, lat, lon), hPa

// Central-difference gradient (hPa / grid-cell), NumPy `np.gradient` style
fn gradient_1d(vals: &[f64]) -> Vec<f64> {
    let n = vals.len();
    if n < 2 { return vec![0.0; n]; }
    let mut out = vec![0.0; n];
    out[0] = vals[1] - vals[0];
    out[n - 1] = vals[n - 1] - vals[n - 2];
    for i in 1..n - 1 {
        out[i] = (vals[i + 1] - vals[i - 1]) / 2.0;
    }
    out
}

// Pressure tendency (hPa / 3h) at one point: central differences in time
let tendency = gradient_1d(&prmsl_point); // already spaced 3 h apart per `hours`

// Anomaly from the first step
let p_anom: Vec<f64> = prmsl_point.iter().map(|p| p - prmsl_point[0]).collect();
```

#### Precipitation: accumulation, exceedance

```rust
// prate is kg/m^2/s == mm/s; * 3600 -> mm/h, then * timestep(h) to accumulate
let dt_hours = 3.0;
let precip_rate_mm_h: Vec<f64> = prate.iter().map(|p| p * 3600.0).collect();

let mut precip_accum_mm = Vec::with_capacity(precip_rate_mm_h.len());
let mut running = 0.0;
for r in &precip_rate_mm_h {
    running += r * dt_hours;
    precip_accum_mm.push(running);
}

// Probability of exceeding 5 mm/h, spatial domain at each timestep
// prate_all: Array3<f64> (time, lat, lon)
let prob_5mm: Vec<f64> = prate_all.outer_iter().map(|slice| {
    let total = slice.len() as f64;
    let hits = slice.iter().filter(|v| *v * 3600.0 > 5.0).count() as f64;
    hits / total
}).collect();
```

#### CAPE: instability classification

```rust
// cape_now: Array2<f64> (lat, lon), J/kg, at one forecast hour
fn cape_category(v: f64) -> u8 {
    if v < 300.0 { 0 }       // marginal
    else if v < 1000.0 { 1 } // moderate
    else if v < 2500.0 { 2 } // large
    else { 3 }               // extreme
}

// Area fraction with extreme instability (CAPE > 2500 J/kg) per timestep
// cape_all: Array3<f64> (time, lat, lon)
let frac_extreme: Vec<f64> = cape_all.outer_iter().map(|slice| {
    let total = slice.len() as f64;
    let hits = slice.iter().filter(|v| **v > 2500.0).count() as f64;
    hits / total
}).collect();
```

#### Upper-air multi-level variables: vertical profiles

```rust
let ds_ua = load(&date, &cycle, vec!["t".into(), "r".into(), "gh".into(), "u".into(), "v".into()],
    vec![0, 24, 48], region.clone())?;

// levels, T_prof, gh_prof, u_prof, v_prof extracted from `ds_ua` at one
// point / one forecast hour (via GetNoaaData::get_data_from_point instead,
// then index into the "level" dim of the returned SelectedVariable).

const R: f64 = 287.05;
const G: f64 = 9.81;
for i in 0..levels.len() - 1 {
    let t_mean_k = (t_prof[i] + t_prof[i + 1]) / 2.0 + 273.15;
    let dz = (R * t_mean_k / G) * (levels[i] / levels[i + 1]).ln();
    println!("{:>4.0}->{:<4.0} hPa  dz ~ {dz:.0} m", levels[i], levels[i + 1]);
}

// Wind shear (m/s per hPa) between levels
let shear_u: Vec<f64> = u_prof.windows(2).zip(levels.windows(2))
    .map(|(uw, lw)| (uw[1] - uw[0]) / (lw[1] - lw[0]))
    .collect();
```

### GFS variable catalogue

```rust
use noawclg::{VARIABLES, SURFACE_VARS, MULTILEVEL_VARS};

println!("{:?}", VARIABLES.keys().collect::<Vec<_>>()); // all 47 keys
println!("{:?}", *SURFACE_VARS);                          // single-level keys
println!("{:?}", *MULTILEVEL_VARS);                        // multi-level keys

for (key, meta) in VARIABLES.iter() {
    println!("{key:8} {:50} {}", meta.long_name, meta.units);
}
```

### Ocean data: GODAS & ERSST

All ocean data is served via **OPeNDAP**: nothing is downloaded to disk,
access is lazy. Requires the `netcdf-io` feature.

```rust
use noawclg::{open_godas, OceanField, BoundingBox};

let field = open_godas(
    2024,
    "pottmp",         // "pottmp" | "salt" | "ucur" | "vcur" | "sshg"
    Some(200.0),      // nearest depth level; None = all 40 levels
    Some(BoundingBox::new(-10.0, 10.0, 120.0, 290.0)),
)?;
let OceanField::Surface(t200) = field else { unreachable!() };
println!("{:?}", t200.data.dim()); // °C, (time=12, lat, lon)
```

```rust
use noawclg::get_godas;

// All 2020-2024 temperature at 200 m
let da = get_godas(2020, 2024, "pottmp", Some(200.0), None)?; // 5 years concatenated
```

```rust
use noawclg::{get_ocean_temp, get_salinity, get_currents, get_ssh};

let t200 = get_ocean_temp(2024, 2024, 200.0, None)?; // °C
let t5 = get_ocean_temp(2024, 2024, 5.0, None)?;      // surface temperature

let sal = get_salinity(2024, 2024, 5.0, None)?;        // PSU

let curr = get_currents(2024, 2024, 5.0, None)?;        // ucur, vcur, speed
println!("{}", curr.speed.spatial_mean()[0]);

let ssh = get_ssh(2024, 2024, None)?;                    // m, (time=12, lat, lon)
let ssh5 = get_ssh(2020, 2024, None)?;                   // 5 years concatenated
```

```rust
use noawclg::{open_ersst, BoundingBox};

// Nino 3.4 box, 1950-2024
let sst = open_ersst(
    Some(1950), Some(2024),
    Some(BoundingBox::new(-5.0, 5.0, 190.0, 240.0)),
)?; // °C
```

```rust
use noawclg::get_sst_series;

let sst_godas = get_sst_series(2000, 2024, "3.4", "godas")?; // 1980+
let sst_ersst = get_sst_series(1950, 2024, "3.4", "ersst")?; // 1854+, longer climatology
```

### ENSO diagnostics

```rust
use noawclg::{get_nino_anomaly, get_oni, classify_enso};

// SST anomaly relative to a 1991-2020 climatology
let anom = get_nino_anomaly(2000, 2024, "3.4", 1991, 2020, "ersst")?;

// Oceanic Nino Index (3-month running mean of the anomaly)
let oni = get_oni(2000, 2024, 1991, 2020, "ersst")?;

// Phase classification (CPC ONI rule: >= 5 consecutive seasons)
let phase = classify_enso(&oni, 0.5, 5); // Vec<String> of "El Niño"/"La Niña"/"Neutral"
```

```rust
use noawclg::enso_summary;

let rows = enso_summary(2015, 2024, 1991, 2020, "ersst")?;
for row in rows.iter().rev().take(12) {
    println!("{} {:>7.2} {:>+6.2} {:>+6.2} {}", row.month, row.sst_nino34, row.anom_nino34, row.oni, row.phase);
}
```

Thermocline depth (D20) and Warm Water Volume:

```rust
use noawclg::{get_thermocline_depth, get_warm_water_volume, BoundingBox};

let d20 = get_thermocline_depth(2024, 2024, Some(BoundingBox::new(-30.0, 30.0, 120.0, 290.0)), 20.0)?;
let wwv = get_warm_water_volume(2020, 2024, 20.0, 300.0)?;
```

### Plotting

Not ported. See [Known limitations](#known-limitations--honesty-notes).

## Module map ↔ Python equivalent

| Rust module | Python module | Notes |
|---|---|---|
| `catalog` | `noawclg/catalog.py` | `VARIABLES` (47 entries; the Python README says 43 but undercounts its own `catalog.py`), `HOURS_*` |
| `coords` | `noawclg/coords.py` | `BoundingBox`, `auto_date`, `find_dim`, `normalize_lon`, `parse_date` |
| `http` | `noawclg/http.py` | NOMADS grib-filter URL builder + HTTP client |
| `gfs_dataset` | `noawclg/gfs_dataset.py` | Download/cache always on; decoding (`build_dataset`/`build_multi_dataset`) needs `grib` |
| `grib_decode` | (`cfgrib` calls inside `gfs_dataset.py`) | GRIB2 → array decoding via `gribberish`, only compiled with `grib` |
| `persistence` | `noawclg/persistence.py` | Zarr v2 (always on, self-contained writer/reader); NetCDF4 needs `netcdf-io` |
| `ocean` | `noawclg/ocean.py` | Split into pure math (masking, region/depth selection, ENSO indices, always on) and live OPeNDAP fetch (needs `netcdf-io`) |
| `query` | `noawclg/query.py` | `GetNoaaData`, Nominatim geocoding (plain HTTP, no `geopy` needed) |
| `view` | `noawclg/view.py` | `DatasetView` |
| `load` | `noawclg/load.py` | `load()` one-liner |
| `icon_dataset` | *(none — Rust-only)* | `IconDatasetManager`: download/decode/remap DWD ICON global into a `GfsDataset`, needs `icon` |
| `icon_catalog` | *(none — Rust-only)* | ICON→canonical variable table, ICON's own forecast-hour cadence |
| `icon_grid` | *(none — Rust-only)* | DWD's published nearest-neighbor remap table (icosahedral → 0.25° world grid) |

`plots.py` and `enso_forecast.py` in the Python repo live outside the
`noawclg` package itself (they're top-level scripts, not part of the
importable library), so they're out of scope for this port. There is no
Rust equivalent of `matplotlib`/`cartopy` plotting here.

## Known limitations / honesty notes

- **GRIB2 decoding (`grib` feature)** uses [`gribberish`](https://crates.io/crates/gribberish)
  instead of the `cfgrib`/`eccodes` combo the Python library uses. The
  decode path (`src/grib_decode.rs`) is written against gribberish's
  documented `Message` API and compiles cleanly, but has not been
  exercised against a real downloaded GFS file in this environment (no
  network egress to NOMADS from the sandbox this was built in). Treat it
  as a solid starting point, not a guarantee. If a real file decodes
  incorrectly, the likely culprit is level-matching in
  `grib_decode::match_level` (isobaric level units aren't 100% pinned down
  by gribberish's docs) or message ordering.
- **`netcdf-io`/`icon` features**: unlike the note above about `grib`,
  these *were* build- and live-tested end-to-end, including real network
  calls to `opendata.dwd.de` + real `cdo` invocations against the live
  2026-08-01 00Z ICON run (see `tests/icon_integration_tests.rs`) — a
  physical plausibility check (temperature range checks, a São Paulo
  winter sanity check, non-negative derived precip rate) passed, not just
  "it didn't crash." One real mistake this surfaced: the machine this was
  developed on already had `libnetcdf-dev` installed for unrelated
  reasons, which masked that it's a genuine, non-optional build
  requirement (see the feature table above — `netcdf-sys` doesn't vendor
  automatically) until CI's `lint` job, which didn't have it installed,
  caught the gap. Fixed by installing it there too.
- **No plotting.** See above: out of scope by design, not an oversight.
- **DataFrame equivalent.** Rust has no `pandas`/`xarray`. `TimeSeries` (a
  `Vec<NaiveDate>` + `Vec<f64>`) stands in for `pd.Series`;
  `DatasetView::to_table()` stands in for `.to_dataframe()` for the common
  case of single-level variables.

## Testing

```bash
cargo test                        # 139 tests (136 pass + 3 opt-in ignored), no network/system deps
cargo test --features grib        # same suite, type-checks the grib decode path too
cargo test --features full        # + 15 more (icon_catalog/icon_grid/icon_dataset unit tests)
cargo test --features full -- --ignored   # + real network/cdo live tests (NOMADS + DWD)
cargo clippy --all-targets --features full -- -D warnings
```

Two layers, mirroring the Python `tests/` suite one file at a time (plus
the Rust-only `icon` module tests, which have no Python counterpart):

| Rust | Ports (Python) | How |
|---|---|---|
| `src/**/*.rs` `#[cfg(test)] mod tests` (38 tests; 53 with `--features full`) | n/a | Unit tests with access to private internals (e.g. `cache_path`, `mask_and_convert`, `icon_grid`'s remap gather) |
| `tests/catalog_tests.rs` | `test_gfs_dataset.py::TestConstants` | Public API only |
| `tests/gfs_dataset_tests.rs` | `test_gfs_dataset.py::TestInit/TestHelpers/TestDownloadHours/TestEdgeCases` | Public API + `common::FakeFetcher` |
| `tests/query_view_tests.rs` | `test_main.py` | Public API + `GetNoaaData::from_dataset` |
| `tests/ocean_tests.rs` | `test_ocean.py` (index math) | Public API only |
| `tests/persistence_tests.rs` | `test_gfs_dataset.py::TestZarr` | Public API, real on-disk round trip |
| `tests/icon_integration_tests.rs` | *(none — Rust-only)* | Real network + real `cdo`, opt-in (`--ignored`), needs `--features icon` |

The Python suite mocks network calls with `unittest.mock.patch.object(mgr._session,
"get", ...)`. The Rust port achieves the same thing with a real seam
instead of monkeypatching: `GfsDatasetManager` is generic over an
`http::Fetcher` trait, and `tests/common/mod.rs::FakeFetcher` implements it
in-memory (canned status/body per call, or a queue for retry scenarios),
recording every URL requested so tests can assert on it, e.g.
`gfs_dataset_tests.rs::var_params_shared_level_token_deduplicated` checks
the exact grib-filter query string `download_hours` builds, without that
method needing to be `pub`. No test in this crate opens a socket.

## License

GPL-3.0-or-later, matching the original Python package.
