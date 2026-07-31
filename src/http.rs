//! Shared HTTP infrastructure for NOMADS downloads.
//!
//! Direct port of `noawclg/http.py`.

use std::time::Duration;

const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; GFS-downloader/2.0; +https://github.com/reinanbr/noawclg)";

/// NOMADS grib-filter URL template. Accepts multiple `&var_XXX=on&lev_XXX=on`.
pub const FILTER_BASE: &str = "https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25_1hr.pl";

/// Build the grib-filter query URL for one forecast hour.
pub fn filter_url(
    date: &str,
    cycle: &str,
    hour: u32,
    var_params: &str,
    region_params: &str,
) -> String {
    format!(
        "{FILTER_BASE}?dir=/gfs.{date}/{cycle}/atmos&file=gfs.t{cycle}z.pgrb2.0p25.f{hour:03}{var_params}{region_params}",
    )
}

/// Create a blocking [`reqwest::blocking::Client`] pre-configured for NOMADS.
///
/// Applies a browser-like User-Agent header (required: NOMADS blocks the
/// default `reqwest` agent with HTTP 403).
///
/// Mirrors `noawclg.http._build_session`.
pub fn build_client(timeout: Duration) -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .build()
}

/// Abstraction over "do a GET request and give me back `(status, body)`".
///
/// [`GfsDatasetManager`](crate::gfs_dataset::GfsDatasetManager) is generic
/// over this trait instead of hard-wiring `reqwest`, the same way the
/// Python library's tests substitute a `MagicMock` for
/// `GFSDatasetManager._session.get`, except here the substitution is a
/// real, statically-typed seam (see `tests/common/mod.rs::FakeFetcher` in
/// the crate's integration tests) rather than monkeypatching.
pub trait Fetcher: Send + Sync {
    fn get(&self, url: &str) -> crate::error::Result<(u16, Vec<u8>)>;
}

/// The real, network-backed [`Fetcher`], used by default.
pub struct ReqwestFetcher {
    client: reqwest::blocking::Client,
}

impl ReqwestFetcher {
    pub fn new(timeout: Duration) -> reqwest::Result<Self> {
        Ok(ReqwestFetcher {
            client: build_client(timeout)?,
        })
    }
}

impl Fetcher for ReqwestFetcher {
    fn get(&self, url: &str) -> crate::error::Result<(u16, Vec<u8>)> {
        let resp = self.client.get(url).send()?;
        let status = resp.status().as_u16();
        let body = resp.bytes()?.to_vec();
        Ok((status, body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_url_builds_expected_query() {
        let url = filter_url("20260605", "12", 24, "&var_TMP=on", "&subregion=&toplat=10");
        assert!(url.starts_with("https://nomads.ncep.noaa.gov/cgi-bin/filter_gfs_0p25_1hr.pl"));
        assert!(url.contains("dir=/gfs.20260605/12/atmos"));
        assert!(url.contains("file=gfs.t12z.pgrb2.0p25.f024"));
        assert!(url.contains("&var_TMP=on"));
        assert!(url.contains("&subregion=&toplat=10"));
    }
}
