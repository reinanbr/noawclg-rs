//! Save and load [`GfsDataset`]s to/from disk.
//!
//! Direct port of `noawclg/persistence.py`. Zarr save/load is a
//! self-contained, dependency-free Zarr v2 writer/reader (uncompressed,
//! single chunk per array) and works with default features. NetCDF4
//! save/load requires the `netcdf-io` feature (system `libnetcdf`, same
//! requirement the Python library has via `netCDF4`/`h5netcdf`).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, TimeZone, Utc};
use ndarray::IxDyn;
use serde_json::json;

use crate::error::{Error, Result};
use crate::gfs_dataset::GfsDataset;

// ══════════════════════════════════════════════════════════════════════════
// Zarr v2 (always available)
// ══════════════════════════════════════════════════════════════════════════

fn chunk_key(shape: &[usize]) -> String {
    if shape.is_empty() {
        "0".to_string()
    } else {
        vec!["0"; shape.len()].join(".")
    }
}

fn write_f64_array(dir: &Path, name: &str, shape: &[usize], data: &[f64]) -> Result<()> {
    let arr_dir = dir.join(name);
    fs::create_dir_all(&arr_dir)?;
    let zarray = json!({
        "zarr_format": 2,
        "shape": shape,
        "chunks": shape,
        "dtype": "<f8",
        "compressor": null,
        "fill_value": null,
        "filters": null,
        "order": "C",
    });
    fs::write(arr_dir.join(".zarray"), serde_json::to_vec_pretty(&zarray)?)?;
    let mut bytes = Vec::with_capacity(data.len() * 8);
    for v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    fs::write(arr_dir.join(chunk_key(shape)), bytes)?;
    Ok(())
}

fn read_f64_array(dir: &Path, name: &str) -> Result<(Vec<usize>, Vec<f64>)> {
    let arr_dir = dir.join(name);
    let meta: serde_json::Value = serde_json::from_slice(&fs::read(arr_dir.join(".zarray"))?)?;
    let shape: Vec<usize> = meta["shape"]
        .as_array()
        .ok_or_else(|| Error::other("malformed .zarray: missing shape"))?
        .iter()
        .map(|v| v.as_u64().unwrap_or(0) as usize)
        .collect();
    let raw = fs::read(arr_dir.join(chunk_key(&shape)))?;
    let data: Vec<f64> = raw
        .chunks_exact(8)
        .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok((shape, data))
}

/// Save a [`GfsDataset`] as an uncompressed Zarr v2 store (a directory).
///
/// Mirrors `persistence.save_zarr`.
pub fn save_zarr(ds: &GfsDataset, store: impl AsRef<Path>) -> Result<PathBuf> {
    let store = store.as_ref().to_path_buf();
    fs::create_dir_all(&store)?;
    fs::write(store.join(".zgroup"), r#"{"zarr_format": 2}"#)?;

    let attrs: serde_json::Map<String, serde_json::Value> = ds
        .attrs
        .iter()
        .map(|(k, v)| (k.clone(), json!(v)))
        .collect();
    fs::write(
        store.join(".zattrs"),
        serde_json::to_vec_pretty(&serde_json::Value::Object(attrs))?,
    )?;

    let time_secs: Vec<f64> = ds.time.iter().map(|t| t.timestamp() as f64).collect();
    write_f64_array(&store, "time", &[time_secs.len()], &time_secs)?;
    let fhours: Vec<f64> = ds.forecast_hour.iter().map(|h| *h as f64).collect();
    write_f64_array(&store, "forecast_hour", &[fhours.len()], &fhours)?;
    write_f64_array(&store, "latitude", &[ds.latitude.len()], &ds.latitude)?;
    write_f64_array(&store, "longitude", &[ds.longitude.len()], &ds.longitude)?;
    if let Some(level) = &ds.level {
        write_f64_array(&store, "level", &[level.len()], level)?;
    }

    for name in &ds.var_order {
        let Some(v) = ds.variables.get(name) else {
            continue;
        };
        let shape = v.data.shape().to_vec();
        let flat: Vec<f64> = v.data.iter().copied().collect();
        write_f64_array(&store, name, &shape, &flat)?;
        let var_dir = store.join(name);
        let var_attrs =
            json!({ "long_name": v.long_name, "units": v.units, "_ARRAY_DIMENSIONS": v.dims });
        fs::write(
            var_dir.join(".zattrs"),
            serde_json::to_vec_pretty(&var_attrs)?,
        )?;
    }

    let mb = dir_size(&store)? as f64 / 1024.0 / 1024.0;
    println!("[save] Zarr  -> {}  ({:.1} MB)", store.display(), mb);
    Ok(store)
}

fn dir_size(dir: &Path) -> Result<u64> {
    let mut total = 0u64;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += meta.len();
        }
    }
    Ok(total)
}

/// Load a previously saved Zarr v2 store back into a [`GfsDataset`].
///
/// Mirrors `persistence.load_zarr`.
pub fn load_zarr(store: impl AsRef<Path>) -> Result<GfsDataset> {
    let store = store.as_ref();
    let mut ds = GfsDataset::default();

    let (_, time_secs) = read_f64_array(store, "time")?;
    ds.time = time_secs
        .into_iter()
        .map(|s| Utc.timestamp_opt(s as i64, 0).unwrap())
        .collect::<Vec<DateTime<Utc>>>();
    let (_, fhours) = read_f64_array(store, "forecast_hour")?;
    ds.forecast_hour = fhours.into_iter().map(|h| h as u32).collect();
    let (_, lat) = read_f64_array(store, "latitude")?;
    ds.latitude = lat;
    let (_, lon) = read_f64_array(store, "longitude")?;
    ds.longitude = lon;
    if store.join("level").join(".zarray").exists() {
        let (_, level) = read_f64_array(store, "level")?;
        ds.level = Some(level);
    }

    let skip = ["time", "forecast_hour", "latitude", "longitude", "level"];
    for entry in fs::read_dir(store)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if skip.contains(&name.as_str()) {
            continue;
        }
        let (shape, flat) = read_f64_array(store, &name)?;
        let data = ndarray::ArrayD::from_shape_vec(IxDyn(&shape), flat)
            .map_err(|e| Error::other(e.to_string()))?;

        let attrs_path = store.join(&name).join(".zattrs");
        let (long_name, units, dims) = if attrs_path.exists() {
            let v: serde_json::Value = serde_json::from_slice(&fs::read(attrs_path)?)?;
            let long_name = v["long_name"].as_str().unwrap_or_default().to_string();
            let units = v["units"].as_str().unwrap_or_default().to_string();
            let dims = v["_ARRAY_DIMENSIONS"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            (long_name, units, dims)
        } else {
            (String::new(), String::new(), Vec::new())
        };

        ds.var_order.push(name.clone());
        ds.variables.insert(
            name,
            crate::gfs_dataset::GfsVariable {
                data,
                dims,
                long_name,
                units,
            },
        );
    }

    println!("[load] {}", store.display());
    Ok(ds)
}

// ══════════════════════════════════════════════════════════════════════════
// NetCDF4 (requires the `netcdf-io` feature + system libnetcdf)
// ══════════════════════════════════════════════════════════════════════════

/// Save a [`GfsDataset`] to a NetCDF4 file. Requires the `netcdf-io` feature.
///
/// Mirrors `persistence.save_netcdf`.
#[cfg(feature = "netcdf-io")]
pub fn save_netcdf(ds: &GfsDataset, filename: impl AsRef<Path>) -> Result<PathBuf> {
    let path = filename.as_ref().to_path_buf();
    let mut file = netcdf::create(&path).map_err(|e| Error::other(e.to_string()))?;

    file.add_dimension("time", ds.time.len())
        .map_err(|e| Error::other(e.to_string()))?;
    file.add_dimension("latitude", ds.latitude.len())
        .map_err(|e| Error::other(e.to_string()))?;
    file.add_dimension("longitude", ds.longitude.len())
        .map_err(|e| Error::other(e.to_string()))?;
    if let Some(level) = &ds.level {
        file.add_dimension("level", level.len())
            .map_err(|e| Error::other(e.to_string()))?;
    }

    {
        let mut var = file
            .add_variable::<f64>("latitude", &["latitude"])
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_values(&ds.latitude, ..)
            .map_err(|e| Error::other(e.to_string()))?;
    }
    {
        let mut var = file
            .add_variable::<f64>("longitude", &["longitude"])
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_values(&ds.longitude, ..)
            .map_err(|e| Error::other(e.to_string()))?;
    }
    {
        let secs: Vec<f64> = ds.time.iter().map(|t| t.timestamp() as f64).collect();
        let mut var = file
            .add_variable::<f64>("time", &["time"])
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_values(&secs, ..)
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_attribute("units", "seconds since 1970-01-01 00:00:00")
            .map_err(|e| Error::other(e.to_string()))?;
    }
    if let Some(level) = &ds.level {
        let mut var = file
            .add_variable::<f64>("level", &["level"])
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_values(level, ..)
            .map_err(|e| Error::other(e.to_string()))?;
    }

    for name in &ds.var_order {
        let Some(v) = ds.variables.get(name) else {
            continue;
        };
        let dim_names: Vec<&str> = v.dims.iter().map(|s| s.as_str()).collect();
        let mut var = file
            .add_variable::<f64>(name, &dim_names)
            .map_err(|e| Error::other(e.to_string()))?;
        let flat: Vec<f64> = v.data.iter().copied().collect();
        var.put_values(&flat, ..)
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_attribute("long_name", v.long_name.as_str())
            .map_err(|e| Error::other(e.to_string()))?;
        var.put_attribute("units", v.units.as_str())
            .map_err(|e| Error::other(e.to_string()))?;
    }

    let mb = fs::metadata(&path)?.len() as f64 / 1024.0 / 1024.0;
    println!("[save] NetCDF -> {}  ({:.1} MB)", path.display(), mb);
    Ok(path)
}

/// Load a previously saved NetCDF4 file back into a [`GfsDataset`].
/// Requires the `netcdf-io` feature.
///
/// Mirrors `persistence.load_netcdf`.
#[cfg(feature = "netcdf-io")]
pub fn load_netcdf(path: impl AsRef<Path>) -> Result<GfsDataset> {
    let path = path.as_ref();
    let file = netcdf::open(path).map_err(|e| Error::other(e.to_string()))?;
    let mut ds = GfsDataset::default();

    let read_dim = |name: &str| -> Result<Vec<f64>> {
        let var = file
            .variable(name)
            .ok_or_else(|| Error::other(format!("missing variable '{name}'")))?;
        var.get_values::<f64, _>(..)
            .map_err(|e| Error::other(e.to_string()))
    };

    ds.latitude = read_dim("latitude")?;
    ds.longitude = read_dim("longitude")?;
    ds.time = read_dim("time")?
        .into_iter()
        .map(|s| Utc.timestamp_opt(s as i64, 0).unwrap())
        .collect();
    if file.variable("level").is_some() {
        ds.level = Some(read_dim("level")?);
    }

    let skip = ["time", "latitude", "longitude", "level"];
    for var in file.variables() {
        let name = var.name();
        if skip.contains(&name.as_str()) {
            continue;
        }
        let dims: Vec<String> = var
            .dimensions()
            .iter()
            .map(|d| d.name().to_string())
            .collect();
        let shape: Vec<usize> = var.dimensions().iter().map(|d| d.len()).collect();
        let flat: Vec<f64> = var
            .get_values::<f64, _>(..)
            .map_err(|e| Error::other(e.to_string()))?;
        let data = ndarray::ArrayD::from_shape_vec(IxDyn(&shape), flat)
            .map_err(|e| Error::other(e.to_string()))?;
        let long_name = var
            .attribute("long_name")
            .and_then(|a| a.value().ok())
            .map(|v| format!("{v:?}"))
            .unwrap_or_default();
        let units = var
            .attribute("units")
            .and_then(|a| a.value().ok())
            .map(|v| format!("{v:?}"))
            .unwrap_or_default();
        ds.var_order.push(name.clone());
        ds.variables.insert(
            name,
            crate::gfs_dataset::GfsVariable {
                data,
                dims,
                long_name,
                units,
            },
        );
    }

    println!("[load] {}", path.display());
    Ok(ds)
}

#[cfg(not(feature = "netcdf-io"))]
pub fn save_netcdf(_ds: &GfsDataset, _filename: impl AsRef<Path>) -> Result<PathBuf> {
    Err(Error::FeatureDisabled("netcdf-io"))
}

#[cfg(not(feature = "netcdf-io"))]
pub fn load_netcdf(_path: impl AsRef<Path>) -> Result<GfsDataset> {
    Err(Error::FeatureDisabled("netcdf-io"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfs_dataset::GfsVariable;
    use ndarray::ArrayD;

    fn sample_dataset() -> GfsDataset {
        let mut ds = GfsDataset {
            time: vec![Utc.with_ymd_and_hms(2026, 4, 3, 0, 0, 0).unwrap()],
            forecast_hour: vec![0],
            latitude: vec![-4.0, -3.0, -2.0],
            longitude: vec![320.0, 321.0, 322.0],
            ..Default::default()
        };
        let data =
            ArrayD::from_shape_vec(IxDyn(&[1, 3, 3]), (0..9).map(|x| x as f64).collect()).unwrap();
        ds.var_order.push("t2m".to_string());
        ds.variables.insert(
            "t2m".to_string(),
            GfsVariable {
                data,
                dims: vec!["time".into(), "latitude".into(), "longitude".into()],
                long_name: "2 metre temperature".into(),
                units: "C".into(),
            },
        );
        ds
    }

    #[test]
    fn zarr_round_trip_preserves_data() {
        let dir = std::env::temp_dir().join(format!("noawclg-zarr-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let ds = sample_dataset();
        save_zarr(&ds, &dir).unwrap();
        let loaded = load_zarr(&dir).unwrap();

        assert_eq!(loaded.latitude, ds.latitude);
        assert_eq!(loaded.longitude, ds.longitude);
        assert_eq!(loaded.time.len(), 1);
        let orig = &ds.variables["t2m"].data;
        let round = &loaded.variables["t2m"].data;
        assert_eq!(orig.shape(), round.shape());
        for (a, b) in orig.iter().zip(round.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
        assert_eq!(loaded.variables["t2m"].long_name, "2 metre temperature");
        let _ = fs::remove_dir_all(&dir);
    }
}
