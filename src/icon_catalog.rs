//! DWD ICON global variable catalogue.
//!
//! Deliberately small and hand-written (unlike [`crate::catalog`]'s 47-entry
//! declarative NOMADS table): ICON global's open-data feed has ~90 single
//! -level fields total, but this only maps the subset with a direct,
//! honest equivalent among the canonical keys [`crate::catalog::VARIABLES`]
//! already uses for GFS, so a [`crate::gfs_dataset::GfsDataset`] built from
//! either model exposes the *same* variable keys — nothing downstream needs
//! to know or care which model actually produced the data.
//!
//! Left out on purpose, rather than faked: GFS's categorical precipitation
//! -type flags (`crain`/`csnow`/`cfrzr`/`cicep`, NCEP code table 4.222)
//! have no equivalent single-level field in ICON's open-data feed — the
//! closest fields (`RAIN_GSP`/`SNOW_GSP`/...) are accumulated amounts, not
//! categorical flags, and turning one into the other convincingly needs
//! more than a units conversion. [`crate::weather::classify_condition`] (in
//! the example/consuming application, not this crate) already degrades
//! gracefully when they're simply absent, so that's what happens here
//! instead of guessing.

/// One ICON→canonical field mapping.
#[derive(Debug, Clone, Copy)]
pub struct IconVarConfig {
    /// Canonical key, matching a [`crate::catalog::VARIABLES`] key exactly
    /// (e.g. `"t2m"`) so callers can request the same names for either
    /// model.
    pub canonical: &'static str,
    /// DWD's directory name under `.../icon/grib/{cycle}/`, e.g. `"t_2m"`.
    /// The GRIB2 file's variable token is always this uppercased (verified
    /// against the live DWD open-data index: `t_2m` ↔ `T_2M`, `vmax_10m` ↔
    /// `VMAX_10M`, etc. — no exceptions found).
    pub dwd_name: &'static str,
    pub long_name: &'static str,
    pub units: &'static str,
    pub converter: Option<fn(f64) -> f64>,
    /// True if the raw DWD field is a run-start-relative accumulation
    /// (currently only `TOT_PREC`, kg/m² accumulated since forecast hour 0)
    /// rather than an instantaneous value, and needs the special
    /// backward-difference handling in `icon_dataset` instead of a plain
    /// per-hour passthrough.
    pub accumulated: bool,
}

fn k_to_c(x: f64) -> f64 {
    x - 273.15
}
fn pa_to_hpa(x: f64) -> f64 {
    x / 100.0
}

/// Every canonical key this module can source from ICON, in a stable order.
pub const ICON_VARIABLES: &[IconVarConfig] = &[
    IconVarConfig {
        canonical: "t2m",
        dwd_name: "t_2m",
        long_name: "2 metre temperature",
        units: "C",
        converter: Some(k_to_c),
        accumulated: false,
    },
    IconVarConfig {
        canonical: "d2m",
        dwd_name: "td_2m",
        long_name: "2 metre dewpoint temperature",
        units: "C",
        converter: Some(k_to_c),
        accumulated: false,
    },
    IconVarConfig {
        canonical: "r2",
        dwd_name: "relhum_2m",
        long_name: "2 metre relative humidity",
        units: "%",
        converter: None,
        accumulated: false,
    },
    IconVarConfig {
        canonical: "u10",
        dwd_name: "u_10m",
        long_name: "10 metre U wind component",
        units: "m s**-1",
        converter: None,
        accumulated: false,
    },
    IconVarConfig {
        canonical: "v10",
        dwd_name: "v_10m",
        long_name: "10 metre V wind component",
        units: "m s**-1",
        converter: None,
        accumulated: false,
    },
    IconVarConfig {
        canonical: "gust",
        // Maximum 10 m wind gust since the previous output step — not
        // published at forecast hour 0 (there's no preceding interval yet).
        // `icon_dataset` falls back to sustained wind speed (hypot(u10,
        // v10)) for hours where this is missing, rather than leaving a gap.
        dwd_name: "vmax_10m",
        long_name: "Wind speed (gust)",
        units: "m s**-1",
        converter: None,
        accumulated: false,
    },
    IconVarConfig {
        canonical: "prmsl",
        dwd_name: "pmsl",
        long_name: "Pressure reduced to MSL",
        units: "hPa",
        converter: Some(pa_to_hpa),
        accumulated: false,
    },
    IconVarConfig {
        canonical: "tcc",
        dwd_name: "clct",
        long_name: "Total cloud cover",
        units: "%",
        converter: None,
        accumulated: false,
    },
    IconVarConfig {
        canonical: "cape",
        // Mixed-layer CAPE — GFS's `cape` is surface-based; both are
        // "how unstable is it right now" in J/kg, close enough to share a
        // canonical key without a converter.
        dwd_name: "cape_ml",
        long_name: "Convective available potential energy",
        units: "J kg**-1",
        converter: None,
        accumulated: false,
    },
    IconVarConfig {
        canonical: "prate",
        // TOT_PREC is *accumulated* precip (kg/m², i.e. mm) since forecast
        // hour 0, not a rate. `icon_dataset::build_dataset` downloads it
        // like any other field but assembles the canonical `prate` (mm/h)
        // by backward-differencing consecutive requested hours instead of
        // applying `converter` directly.
        dwd_name: "tot_prec",
        long_name: "Precipitation rate",
        units: "kg m**-2 s**-1",
        converter: None,
        accumulated: true,
    },
];

/// Look up one canonical key's ICON mapping.
pub fn get(canonical: &str) -> Option<&'static IconVarConfig> {
    ICON_VARIABLES.iter().find(|c| c.canonical == canonical)
}

/// Every canonical key this module can source from ICON.
pub fn canonical_keys() -> Vec<&'static str> {
    ICON_VARIABLES.iter().map(|c| c.canonical).collect()
}

/// `0..=78` @ 1 h, then `81..=180` @ 3 h — ICON global's own published
/// forecast-hour cadence for the 00Z/12Z runs (verified against the live
/// DWD open-data index for the `t_2m` field). 06Z/18Z runs only publish out
/// to 120 h but follow the same cadence over that shorter range, so hours
/// beyond what a given run actually has just won't be found and are
/// silently skipped by [`crate::icon_dataset::IconDatasetManager`], the
/// same way [`crate::gfs_dataset::GfsDatasetManager::download_hours`]
/// already treats missing hours as "not published (yet)" rather than an
/// error.
pub fn default_hours() -> Vec<u32> {
    (0..=78).chain((81..=180).step_by(3)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_keys_match_gfs_catalog_keys() {
        // Every ICON canonical key must exist in the GFS catalogue too —
        // that's the whole point (same key, either model) — except this
        // crate's GFS catalogue doesn't need to have *every* ICON key
        // covered, just that ICON never invents a key GFS doesn't know.
        for key in canonical_keys() {
            assert!(
                crate::catalog::VARIABLES.contains_key(key),
                "ICON canonical key '{key}' has no matching GFS catalogue entry"
            );
        }
    }

    #[test]
    fn t2m_converts_kelvin_to_celsius() {
        let cfg = get("t2m").unwrap();
        assert_eq!((cfg.converter.unwrap())(273.15), 0.0);
    }

    #[test]
    fn prate_is_flagged_accumulated() {
        assert!(get("prate").unwrap().accumulated);
        assert!(!get("t2m").unwrap().accumulated);
    }

    #[test]
    fn get_returns_none_for_unmapped_key() {
        assert!(get("crain").is_none());
    }

    #[test]
    fn default_hours_has_113_steps_ending_at_180() {
        let hours = default_hours();
        assert_eq!(hours.len(), 113);
        assert_eq!(hours[0], 0);
        assert_eq!(*hours.last().unwrap(), 180);
    }
}
