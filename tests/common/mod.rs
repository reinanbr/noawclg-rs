//! Shared test support: a canned, in-memory [`noawclg::http::Fetcher`].
//!
//! This is the Rust equivalent of the `unittest.mock.patch.object(mgr._session,
//! "get", ...)` pattern used throughout `tests/test_gfs_dataset.py` in the
//! Python repo — except here the substitution point is a real trait
//! (`noawclg::http::Fetcher`) rather than monkeypatching a method, so it's
//! checked by the compiler.

use std::sync::{Arc, Mutex};

use noawclg::http::Fetcher;

/// One canned HTTP response.
#[derive(Clone)]
pub struct Canned {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Canned {
    pub fn ok(body: impl Into<Vec<u8>>) -> Self {
        Canned {
            status: 200,
            body: body.into(),
        }
    }

    pub fn status(status: u16) -> Self {
        Canned {
            status,
            body: Vec::new(),
        }
    }
}

/// Shared handle onto a [`FakeFetcher`]'s call log — keep this before
/// moving the fetcher into a `Box<dyn Fetcher>` so you can still inspect
/// what was requested afterwards (mirrors asserting on `mock_get.call_args`
/// in the Python tests).
#[derive(Clone, Default)]
pub struct CallLog(Arc<Mutex<Vec<String>>>);

impl CallLog {
    pub fn urls(&self) -> Vec<String> {
        self.0.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

/// A [`Fetcher`] that never touches the network: it either returns
/// responses from a fixed queue (consumed in order, 404 once exhausted), a
/// single fixed response for every call, or an error for every call. Every
/// requested URL is recorded in a [`CallLog`] so tests can assert on it.
pub struct FakeFetcher {
    queue: Mutex<Vec<Canned>>,
    default: Option<Canned>,
    fail: bool,
    log: CallLog,
}

impl FakeFetcher {
    /// Every call returns the same response.
    pub fn always(response: Canned) -> Self {
        FakeFetcher {
            queue: Mutex::new(Vec::new()),
            default: Some(response),
            fail: false,
            log: CallLog::default(),
        }
    }

    /// Every call returns a network error (mirrors `requests.RequestException`).
    pub fn failing() -> Self {
        FakeFetcher {
            queue: Mutex::new(Vec::new()),
            default: None,
            fail: true,
            log: CallLog::default(),
        }
    }

    /// Calls consume the queue in order; once empty, falls back to 404.
    pub fn queue(responses: Vec<Canned>) -> Self {
        let mut q = responses;
        q.reverse(); // pop() takes from the back
        FakeFetcher {
            queue: Mutex::new(q),
            default: None,
            fail: false,
            log: CallLog::default(),
        }
    }

    /// A cloneable handle to this fetcher's call log — take this *before*
    /// boxing the fetcher and handing it to `GfsDatasetManager::with_fetcher`.
    pub fn log(&self) -> CallLog {
        self.log.clone()
    }
}

impl Fetcher for FakeFetcher {
    fn get(&self, url: &str) -> noawclg::Result<(u16, Vec<u8>)> {
        self.log.0.lock().unwrap().push(url.to_string());

        if self.fail {
            return Err(noawclg::Error::other("simulated network error"));
        }

        let mut q = self.queue.lock().unwrap();
        let resp = if let Some(next) = q.pop() {
            next
        } else if let Some(d) = &self.default {
            d.clone()
        } else {
            Canned::status(404)
        };
        Ok((resp.status, resp.body))
    }
}

/// A GRIB2-shaped dummy payload: real header/trailer markers so
/// `GfsDatasetManager`'s cache validity check accepts it, padded past the
/// 100-byte minimum-size cutoff.
pub fn dummy_grib_bytes(len: usize) -> Vec<u8> {
    let mut b = vec![b'G'; len.max(8)];
    b[0..4].copy_from_slice(b"GRIB");
    let n = b.len();
    b[n - 4..].copy_from_slice(b"7777");
    b
}
