//! Port of `tests/test_gfs_dataset.py::TestConstants` (Python), exercised
//! here purely through `noawclg`'s public API.

use noawclg::{
    HOURS_10DAYS_3H, HOURS_16DAYS, HOURS_16DAYS_3H, HOURS_5DAYS_1H, MULTILEVEL_VARS, SURFACE_VARS,
    VARIABLES,
};

#[test]
fn variables_not_empty() {
    assert!(!VARIABLES.is_empty());
}

#[test]
fn surface_vars_are_subset() {
    assert!(SURFACE_VARS.iter().all(|k| VARIABLES.contains_key(k)));
}

#[test]
fn multilevel_vars_are_subset() {
    assert!(MULTILEVEL_VARS.iter().all(|k| VARIABLES.contains_key(k)));
}

#[test]
fn surface_and_multilevel_disjoint() {
    assert!(SURFACE_VARS.iter().all(|k| !MULTILEVEL_VARS.contains(k)));
}

#[test]
fn all_vars_covered_by_subsets() {
    let mut combined: Vec<&str> = SURFACE_VARS
        .iter()
        .chain(MULTILEVEL_VARS.iter())
        .copied()
        .collect();
    combined.sort_unstable();
    combined.dedup();
    let mut all: Vec<&str> = VARIABLES.keys().copied().collect();
    all.sort_unstable();
    assert_eq!(combined, all);
}

#[test]
fn surface_vars_have_no_multilevel_flag() {
    for key in SURFACE_VARS.iter() {
        assert!(!VARIABLES[key].multilevel, "{key}");
    }
}

#[test]
fn multilevel_vars_have_flag() {
    for key in MULTILEVEL_VARS.iter() {
        assert!(VARIABLES[key].multilevel, "{key}");
    }
}

#[test]
fn t2m_converter_k_to_c() {
    let conv = VARIABLES["t2m"].converter.unwrap();
    assert!((conv(273.15) - 0.0).abs() < 1e-9);
}

#[test]
fn prmsl_converter_pa_to_hpa() {
    let conv = VARIABLES["prmsl"].converter.unwrap();
    assert!((conv(101_325.0) - 1013.25).abs() < 1e-9);
}

#[test]
fn hours_16days_start_end() {
    assert_eq!(HOURS_16DAYS[0], 0);
    assert_eq!(*HOURS_16DAYS.last().unwrap(), 384);
}

#[test]
fn hours_5days_1h_length() {
    assert_eq!(HOURS_5DAYS_1H.len(), 121);
}

#[test]
fn hours_10days_3h_constant_step() {
    let diffs: Vec<u32> = HOURS_10DAYS_3H.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(diffs.iter().all(|&d| d == 3));
}

#[test]
fn hours_16days_3h_sorted() {
    let mut sorted = HOURS_16DAYS_3H.clone();
    sorted.sort_unstable();
    assert_eq!(sorted, *HOURS_16DAYS_3H);
}

#[test]
fn grib_var_prefix() {
    for (key, cfg) in VARIABLES.iter() {
        assert!(cfg.grib_var.starts_with("var_"), "{key}");
    }
}

#[test]
fn grib_lev_prefix() {
    for (key, cfg) in VARIABLES.iter() {
        assert!(cfg.grib_lev.starts_with("lev_"), "{key}");
    }
}

#[test]
fn common_surface_vars_exist() {
    for key in ["t2m", "prmsl", "prate", "u10", "v10"] {
        assert!(SURFACE_VARS.contains(&key), "{key}");
    }
}

#[test]
fn common_multilevel_vars_exist() {
    for key in ["t", "gh", "u", "v", "w"] {
        assert!(MULTILEVEL_VARS.contains(&key), "{key}");
    }
}
