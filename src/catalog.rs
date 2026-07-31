//! GFS variable catalogue and pre-defined forecast-hour sequences.
//!
//! Direct port of `noawclg/catalog.py`.

use std::collections::HashMap;
use std::sync::LazyLock;

/// Metadata for one GFS variable: how to fetch it from NOMADS and how to
/// convert the raw GRIB2 value into human-friendly units.
#[derive(Debug, Clone, Copy)]
pub struct VarConfig {
    pub short: &'static str,
    pub long_name: &'static str,
    pub units: &'static str,
    /// GRIB2 `typeOfLevel` (e.g. "heightAboveGround", "isobaricInhPa").
    pub tlev: &'static str,
    /// Levels for multilevel variables (hPa for isobaric, cm for soil, m for height).
    pub levels: Option<&'static [i32]>,
    /// NOMADS grib-filter query token, e.g. "var_TMP".
    pub grib_var: &'static str,
    /// NOMADS grib-filter level token, e.g. "lev_2_m_above_ground".
    pub grib_lev: &'static str,
    /// Per-level NOMADS level token override (soil layers).
    pub grib_lev_map: Option<&'static [(i32, &'static str)]>,
    /// Unit converter applied to every raw value after decoding.
    pub converter: Option<fn(f64) -> f64>,
    /// True if the variable carries a `level` dimension.
    pub multilevel: bool,
    /// True if the variable only lives in the secondary (pgrb2b) file.
    pub pgrb2b_only: bool,
    /// Alternate NOMADS token when the primary token differs in pgrb2b.
    pub pgrb2b_var: Option<&'static str>,
    /// True if only present at forecast hour 0 (the analysis step).
    pub f000_only: bool,
}

impl Default for VarConfig {
    fn default() -> Self {
        VarConfig {
            short: "",
            long_name: "",
            units: "",
            tlev: "surface",
            levels: None,
            grib_var: "",
            grib_lev: "",
            grib_lev_map: None,
            converter: None,
            multilevel: false,
            pgrb2b_only: false,
            pgrb2b_var: None,
            f000_only: false,
        }
    }
}

fn k_to_c(x: f64) -> f64 {
    x - 273.15
}
fn pa_to_hpa(x: f64) -> f64 {
    x / 100.0
}

macro_rules! var {
    ($($field:ident : $value:expr),* $(,)?) => {
        VarConfig { $($field: $value,)* ..VarConfig::default() }
    };
}

/// Full GFS variable catalogue: maps short name to metadata.
pub static VARIABLES: LazyLock<HashMap<&'static str, VarConfig>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // ── 2-metre / surface single-level ──────────────────────────────────
    m.insert(
        "t2m",
        var! {
            short: "t2m", long_name: "2 metre temperature", units: "C",
            tlev: "heightAboveGround", levels: Some(&[2]),
            grib_var: "var_TMP", grib_lev: "lev_2_m_above_ground",
            converter: Some(k_to_c),
        },
    );
    m.insert(
        "d2m",
        var! {
            short: "d2m", long_name: "2 metre dewpoint temperature", units: "C",
            tlev: "heightAboveGround", levels: Some(&[2]),
            grib_var: "var_DPT", grib_lev: "lev_2_m_above_ground",
            converter: Some(k_to_c),
        },
    );
    m.insert(
        "r2",
        var! {
            short: "r2", long_name: "2 metre relative humidity", units: "%",
            tlev: "heightAboveGround", levels: Some(&[2]),
            grib_var: "var_RH", grib_lev: "lev_2_m_above_ground",
        },
    );
    m.insert(
        "sh2",
        var! {
            short: "sh2", long_name: "2 metre specific humidity", units: "kg kg**-1",
            tlev: "heightAboveGround", levels: Some(&[2]),
            grib_var: "var_SPFH", grib_lev: "lev_2_m_above_ground",
        },
    );
    m.insert(
        "aptmp",
        var! {
            short: "aptmp", long_name: "Apparent temperature", units: "C",
            tlev: "heightAboveGround", levels: Some(&[2]),
            grib_var: "var_APTMP", grib_lev: "lev_2_m_above_ground",
            converter: Some(k_to_c), pgrb2b_only: true,
        },
    );

    // ── 10-metre wind ────────────────────────────────────────────────────
    m.insert(
        "u10",
        var! {
            short: "u10", long_name: "10 metre U wind component", units: "m s**-1",
            tlev: "heightAboveGround", levels: Some(&[10]),
            grib_var: "var_UGRD", grib_lev: "lev_10_m_above_ground",
        },
    );
    m.insert(
        "v10",
        var! {
            short: "v10", long_name: "10 metre V wind component", units: "m s**-1",
            tlev: "heightAboveGround", levels: Some(&[10]),
            grib_var: "var_VGRD", grib_lev: "lev_10_m_above_ground",
        },
    );
    m.insert(
        "gust",
        var! {
            short: "gust", long_name: "Wind speed (gust)", units: "m s**-1",
            tlev: "surface",
            grib_var: "var_GUST", grib_lev: "lev_surface",
        },
    );

    // ── surface / single-level ───────────────────────────────────────────
    m.insert(
        "prmsl",
        var! {
            short: "prmsl", long_name: "Pressure reduced to MSL", units: "hPa",
            tlev: "meanSea",
            grib_var: "var_PRMSL", grib_lev: "lev_mean_sea_level",
            converter: Some(pa_to_hpa),
        },
    );
    m.insert(
        "mslet",
        var! {
            short: "mslet", long_name: "MSLP (Eta model reduction)", units: "hPa",
            tlev: "meanSea",
            grib_var: "var_PRMSL", grib_lev: "lev_mean_sea_level",
            converter: Some(pa_to_hpa), pgrb2b_var: Some("var_MSLET"),
        },
    );
    m.insert(
        "sp",
        var! {
            short: "sp", long_name: "Surface pressure", units: "hPa",
            tlev: "surface",
            grib_var: "var_PRES", grib_lev: "lev_surface",
            converter: Some(pa_to_hpa),
        },
    );
    m.insert(
        "orog",
        var! {
            short: "orog", long_name: "Orography", units: "m",
            tlev: "surface",
            grib_var: "var_HGT", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "lsm",
        var! {
            short: "lsm", long_name: "Land-sea mask", units: "0 - 1",
            tlev: "surface",
            grib_var: "var_LAND", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "vis",
        var! {
            short: "vis", long_name: "Visibility", units: "m",
            tlev: "surface",
            grib_var: "var_VIS", grib_lev: "lev_surface",
        },
    );

    // ── precipitation / hydrology ────────────────────────────────────────
    m.insert(
        "prate",
        var! {
            short: "prate", long_name: "Precipitation rate", units: "kg m**-2 s**-1",
            tlev: "surface",
            grib_var: "var_PRATE", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "cpofp",
        var! {
            short: "cpofp", long_name: "Percent frozen precipitation", units: "%",
            tlev: "surface",
            grib_var: "var_CPOFP", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "crain",
        var! {
            short: "crain", long_name: "Categorical rain", units: "Code table 4.222",
            tlev: "surface",
            grib_var: "var_CRAIN", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "csnow",
        var! {
            short: "csnow", long_name: "Categorical snow", units: "Code table 4.222",
            tlev: "surface",
            grib_var: "var_CSNOW", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "cfrzr",
        var! {
            short: "cfrzr", long_name: "Categorical freezing rain", units: "Code table 4.222",
            tlev: "surface",
            grib_var: "var_CFRZR", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "cicep",
        var! {
            short: "cicep", long_name: "Categorical ice pellets", units: "Code table 4.222",
            tlev: "surface",
            grib_var: "var_CICEP", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "sde",
        var! {
            short: "sde", long_name: "Snow depth", units: "m",
            tlev: "surface",
            grib_var: "var_SNOD", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "sdwe",
        var! {
            short: "sdwe", long_name: "Water equivalent of accumulated snow depth", units: "kg m**-2",
            tlev: "surface",
            grib_var: "var_WEASD", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "pwat",
        var! {
            short: "pwat", long_name: "Precipitable water", units: "kg m**-2",
            tlev: "atmosphereSingleLayer",
            grib_var: "var_PWAT",
            grib_lev: "lev_entire_atmosphere_(considered_as_a_single_layer)",
        },
    );
    m.insert(
        "cwat",
        var! {
            short: "cwat", long_name: "Cloud water", units: "kg m**-2",
            tlev: "atmosphereSingleLayer",
            grib_var: "var_CWAT",
            grib_lev: "lev_entire_atmosphere_(considered_as_a_single_layer)",
        },
    );

    // ── cloud cover ──────────────────────────────────────────────────────
    m.insert(
        "tcc",
        var! {
            short: "tcc", long_name: "Total cloud cover", units: "%",
            tlev: "atmosphere",
            grib_var: "var_TCDC", grib_lev: "lev_entire_atmosphere",
        },
    );
    m.insert(
        "lcc",
        var! {
            short: "lcc", long_name: "Low cloud cover", units: "%",
            tlev: "lowCloudLayer",
            grib_var: "var_TCDC", grib_lev: "lev_low_cloud_layer",
            pgrb2b_only: true,
        },
    );
    m.insert(
        "mcc",
        var! {
            short: "mcc", long_name: "Medium cloud cover", units: "%",
            tlev: "middleCloudLayer",
            grib_var: "var_TCDC", grib_lev: "lev_middle_cloud_layer",
            pgrb2b_only: true,
        },
    );
    m.insert(
        "hcc",
        var! {
            short: "hcc", long_name: "High cloud cover", units: "%",
            tlev: "highCloudLayer",
            grib_var: "var_TCDC", grib_lev: "lev_high_cloud_layer",
            pgrb2b_only: true,
        },
    );

    // ── convection / instability ─────────────────────────────────────────
    m.insert(
        "cape",
        var! {
            short: "cape", long_name: "Convective available potential energy", units: "J kg**-1",
            tlev: "surface",
            grib_var: "var_CAPE", grib_lev: "lev_surface",
            multilevel: true,
        },
    );
    m.insert(
        "cin",
        var! {
            short: "cin", long_name: "Convective inhibition", units: "J kg**-1",
            tlev: "surface",
            grib_var: "var_CIN", grib_lev: "lev_surface",
            multilevel: true,
        },
    );
    m.insert(
        "lftx",
        var! {
            short: "lftx", long_name: "Surface lifted index", units: "K",
            tlev: "surface",
            grib_var: "var_LFTX", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "lftx4",
        var! {
            short: "lftx4", long_name: "Best (4-layer) lifted index", units: "K",
            tlev: "surface",
            grib_var: "var_4LFTX", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "hlcy",
        var! {
            short: "hlcy", long_name: "Storm relative helicity", units: "m**2 s**-2",
            tlev: "heightAboveGroundLayer",
            grib_var: "var_HLCY", grib_lev: "lev_0-3000_m_above_ground_layer",
        },
    );

    // ── upper-air multi-level (isobaric) ─────────────────────────────────
    m.insert(
        "t",
        var! {
            short: "t", long_name: "Temperature", units: "C",
            tlev: "isobaricInhPa",
            levels: Some(&[80, 100, 150, 200, 250, 300, 400, 500, 600, 700, 850, 925, 1000]),
            grib_var: "var_TMP", grib_lev: "lev_mb",
            converter: Some(k_to_c), multilevel: true,
        },
    );
    m.insert(
        "r",
        var! {
            short: "r", long_name: "Relative humidity", units: "%",
            tlev: "isobaricInhPa",
            levels: Some(&[80, 100, 150, 200, 250, 300, 400, 500, 600, 700, 850, 925, 1000]),
            grib_var: "var_RH", grib_lev: "lev_mb",
            multilevel: true,
        },
    );
    m.insert(
        "q",
        var! {
            short: "q", long_name: "Specific humidity", units: "kg kg**-1",
            tlev: "isobaricInhPa", levels: Some(&[80, 1000]),
            grib_var: "var_SPFH", grib_lev: "lev_mb",
            multilevel: true,
        },
    );
    m.insert(
        "gh",
        var! {
            short: "gh", long_name: "Geopotential height", units: "gpm",
            tlev: "isobaricInhPa", levels: Some(&[500, 700, 850, 925, 1000]),
            grib_var: "var_HGT", grib_lev: "lev_mb",
            multilevel: true,
        },
    );
    m.insert(
        "u",
        var! {
            short: "u", long_name: "U component of wind", units: "m s**-1",
            tlev: "isobaricInhPa",
            levels: Some(&[200, 250, 300, 400, 500, 700, 850, 925, 1000]),
            grib_var: "var_UGRD", grib_lev: "lev_mb",
            multilevel: true,
        },
    );
    m.insert(
        "v",
        var! {
            short: "v", long_name: "V component of wind", units: "m s**-1",
            tlev: "isobaricInhPa",
            levels: Some(&[200, 250, 300, 400, 500, 700, 850, 925, 1000]),
            grib_var: "var_VGRD", grib_lev: "lev_mb",
            multilevel: true,
        },
    );
    m.insert(
        "w",
        var! {
            short: "w", long_name: "Vertical velocity", units: "Pa s**-1",
            tlev: "isobaricInhPa",
            levels: Some(&[100, 200, 300, 400, 500, 600, 700, 850]),
            grib_var: "var_VVEL", grib_lev: "lev_mb",
            multilevel: true,
        },
    );
    m.insert(
        "absv",
        var! {
            short: "absv", long_name: "Absolute vorticity", units: "s**-1",
            tlev: "isobaricInhPa",
            levels: Some(&[100, 200, 300, 400, 500, 700, 850, 1000]),
            grib_var: "var_ABSV", grib_lev: "lev_mb",
            multilevel: true,
        },
    );

    // ── soil (4-layer) ───────────────────────────────────────────────────
    const SOIL_LEV_MAP: &[(i32, &str)] = &[
        (0, "lev_0-10_cm_below_ground"),
        (10, "lev_10-40_cm_below_ground"),
        (40, "lev_40-100_cm_below_ground"),
        (100, "lev_100-200_cm_below_ground"),
    ];
    m.insert(
        "st",
        var! {
            short: "st", long_name: "Soil temperature", units: "C",
            tlev: "depthBelowLandLayer", levels: Some(&[0, 10, 40, 100]),
            grib_var: "var_TSOIL", grib_lev: "lev_0-10_cm_below_ground",
            grib_lev_map: Some(SOIL_LEV_MAP),
            converter: Some(k_to_c), multilevel: true,
        },
    );
    m.insert(
        "soilw",
        var! {
            short: "soilw", long_name: "Volumetric soil moisture content", units: "Proportion",
            tlev: "depthBelowLandLayer", levels: Some(&[0, 10, 40, 100]),
            grib_var: "var_SOILW", grib_lev: "lev_0-10_cm_below_ground",
            grib_lev_map: Some(SOIL_LEV_MAP),
            multilevel: true,
        },
    );

    // ── diagnostics ──────────────────────────────────────────────────────
    m.insert(
        "refc",
        var! {
            short: "refc", long_name: "Maximum/Composite radar reflectivity", units: "dB",
            tlev: "atmosphere",
            grib_var: "var_REFC", grib_lev: "lev_entire_atmosphere",
        },
    );
    m.insert(
        "siconc",
        var! {
            short: "siconc", long_name: "Sea ice area fraction", units: "0 - 1",
            tlev: "surface",
            grib_var: "var_ICEC", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "veg",
        var! {
            short: "veg", long_name: "Vegetation", units: "%",
            tlev: "surface",
            grib_var: "var_VEG", grib_lev: "lev_surface",
        },
    );
    m.insert(
        "tozne",
        var! {
            short: "tozne", long_name: "Total ozone", units: "DU",
            tlev: "atmosphere",
            grib_var: "var_TOZNE", grib_lev: "lev_entire_atmosphere",
            f000_only: true,
        },
    );

    m
});

/// Single-level keys (no `level` dimension).
pub static SURFACE_VARS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut v: Vec<&'static str> = VARIABLES
        .iter()
        .filter(|(_, cfg)| !cfg.multilevel)
        .map(|(k, _)| *k)
        .collect();
    v.sort_unstable();
    v
});

/// Multi-level (pressure-level / multi-layer) keys.
pub static MULTILEVEL_VARS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut v: Vec<&'static str> = VARIABLES
        .iter()
        .filter(|(_, cfg)| cfg.multilevel)
        .map(|(k, _)| *k)
        .collect();
    v.sort_unstable();
    v
});

/// 0–120 h @ 6 h, then 123–384 h @ 3 h.
pub static HOURS_16DAYS: LazyLock<Vec<u32>> =
    LazyLock::new(|| (0..=120).step_by(6).chain((123..=384).step_by(3)).collect());

/// 0–120 h, every 1 h.
pub static HOURS_5DAYS_1H: LazyLock<Vec<u32>> = LazyLock::new(|| (0..=120).collect());

/// 0–240 h, every 3 h.
pub static HOURS_10DAYS_3H: LazyLock<Vec<u32>> = LazyLock::new(|| (0..=240).step_by(3).collect());

/// 0–120 h @ 3 h, then 123–384 h @ 3 h.
pub static HOURS_16DAYS_3H: LazyLock<Vec<u32>> =
    LazyLock::new(|| (0..=120).step_by(3).chain((123..=384).step_by(3)).collect());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_has_47_entries() {
        // The Python README advertises "43 keys", but the actual
        // noawclg/catalog.py VARIABLES dict (ported verbatim here) has 47.
        assert_eq!(VARIABLES.len(), 47);
    }

    #[test]
    fn t2m_converts_kelvin_to_celsius() {
        let cfg = VARIABLES["t2m"];
        assert_eq!((cfg.converter.unwrap())(273.15), 0.0);
    }

    #[test]
    fn surface_and_multilevel_partition_all_variables() {
        assert_eq!(SURFACE_VARS.len() + MULTILEVEL_VARS.len(), VARIABLES.len());
    }

    #[test]
    fn hours_5days_1h_has_121_steps() {
        assert_eq!(HOURS_5DAYS_1H.len(), 121);
        assert_eq!(HOURS_5DAYS_1H[0], 0);
        assert_eq!(*HOURS_5DAYS_1H.last().unwrap(), 120);
    }

    #[test]
    fn hours_10days_3h_has_81_steps() {
        assert_eq!(HOURS_10DAYS_3H.len(), 81);
        assert_eq!(*HOURS_10DAYS_3H.last().unwrap(), 240);
    }

    #[test]
    fn hours_16days_3h_has_129_steps() {
        assert_eq!(HOURS_16DAYS_3H.len(), 129);
        assert_eq!(*HOURS_16DAYS_3H.last().unwrap(), 384);
    }
}
