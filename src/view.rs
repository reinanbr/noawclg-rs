//! Lightweight read-only wrapper over a dataset already selected at one
//! grid point.
//!
//! Direct port of `noawclg/view.py::_DatasetView`.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use ndarray::ArrayD;

use crate::error::{Error, Result};

/// One variable's data at a single `(lat, lon)` grid point — dims are
/// `["time"]` for surface variables or `["time", "level"]` for multilevel
/// ones.
#[derive(Debug, Clone)]
pub struct SelectedVariable {
    pub data: ArrayD<f64>,
    pub dims: Vec<String>,
    pub long_name: String,
    pub units: String,
}

/// A [`crate::gfs_dataset::GfsDataset`] selected at the nearest grid point
/// to some `(lat, lon)`.
///
/// Mirrors `noawclg.view._DatasetView`.
#[derive(Debug, Clone)]
pub struct DatasetView {
    pub time: Vec<DateTime<Utc>>,
    pub forecast_hour: Vec<u32>,
    pub latitude: f64,
    pub longitude: f64,
    pub level: Option<Vec<f64>>,
    pub var_order: Vec<String>,
    pub variables: HashMap<String, SelectedVariable>,
}

impl DatasetView {
    /// Fetch one variable by key. Mirrors `_DatasetView.__getitem__`.
    pub fn get(&self, key: &str) -> Result<&SelectedVariable> {
        self.variables
            .get(key)
            .ok_or_else(|| Error::VariableNotFound(key.to_string(), self.var_order.clone()))
    }

    /// Flatten every single-level (`["time"]`-only) variable into a table
    /// of `{column: value}` rows, one per forecast time step — the
    /// practical analogue of `.to_dataframe()` for the common case (a
    /// scalar time series per variable).
    pub fn to_table(&self) -> Vec<BTreeMap<String, f64>> {
        (0..self.time.len())
            .map(|i| {
                let mut row = BTreeMap::new();
                row.insert("forecast_hour".to_string(), self.forecast_hour[i] as f64);
                for name in &self.var_order {
                    if let Some(v) = self.variables.get(name) {
                        if v.dims == ["time"] {
                            row.insert(name.clone(), v.data[[i]]);
                        }
                    }
                }
                row
            })
            .collect()
    }
}
