//! Crate-wide error type.

use std::fmt;

/// Errors produced by `noawclg`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid date '{0}': expected 'DD/MM/YYYY'")]
    InvalidDate(String),

    #[error("cycle must be one of: 00, 06, 12, 18 (got '{0}')")]
    InvalidCycle(String),

    #[error("unknown variable key(s): {0:?}")]
    UnknownVariables(Vec<String>),

    #[error("unknown GODAS variable '{0}'. Choose from: pottmp, salt, ucur, vcur, sshg")]
    UnknownGodasVariable(String),

    #[error("Cannot find a {label} coordinate in the dataset. Tried: {tried:?}. Available: {available:?}")]
    DimensionNotFound {
        label: String,
        tried: Vec<&'static str>,
        available: Vec<String>,
    },

    #[error("Variable '{0}' not found. Available: {1:?}")]
    VariableNotFound(String, Vec<String>),

    #[error("could not geocode '{0}'. Try a more specific name or verify the spelling.")]
    GeocodeNotFound(String),

    #[error("no files available for hours={hours:?} (date={date}, cycle={cycle}). NOMADS may not have published this run yet.")]
    NoFilesAvailable {
        hours: Vec<u32>,
        date: String,
        cycle: String,
    },

    #[error("no valid data found for '{0}'")]
    NoDataForVariable(String),

    #[error("no data loaded for {start}-{end}: {reason}")]
    NoDataForRange {
        start: i32,
        end: i32,
        reason: String,
    },

    #[error("this build was compiled without the '{0}' feature; rebuild with `--features {0}`")]
    FeatureDisabled(&'static str),

    #[error("required system dependency '{0}' was not found on PATH; see the README's ICON section for install instructions")]
    MissingSystemDependency(&'static str),

    #[error("unknown ICON variable key(s): {0:?}. Available: t2m, d2m, r2, u10, v10, gust, prmsl, prate, tcc, cape")]
    UnknownIconVariables(Vec<String>),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn other(msg: impl fmt::Display) -> Self {
        Error::Other(msg.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
