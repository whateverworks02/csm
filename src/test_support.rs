//! Shared test helpers for integration tests that need a private `$CSM_HOME`.
//!
//! Integration tests across modules (`store` CRUD, `workspace` scaffold) mutate
//! the process-global `CSM_HOME` env var, so they must run `#[serial]` and share
//! one isolation helper. Inline `mod tests` blocks can't reach each other's
//! private helpers, so this module is `pub(crate)` - the legitimate case for
//! widening (cross-module test use), as opposed to exposing the unit under test
//! itself, which inline `mod tests` can already reach via `use super::*`.

use std::path::Path;
use tempfile::TempDir;

/// Save the current `CSM_HOME` and restore it on drop - panic-safe.
struct CsmHomeGuard {
    prior: Option<String>,
}

impl CsmHomeGuard {
    fn new() -> Self {
        CsmHomeGuard {
            prior: std::env::var("CSM_HOME").ok(),
        }
    }
}

impl Drop for CsmHomeGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var("CSM_HOME", v),
            None => std::env::remove_var("CSM_HOME"),
        }
    }
}

/// Point `$CSM_HOME` at a fresh temp dir for `f`, restoring it afterwards.
/// Tests using this must be `#[serial]` - concurrent env mutation races.
pub(crate) fn with_csm_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let _guard = CsmHomeGuard::new();
    let dir = TempDir::new().unwrap();
    std::env::set_var("CSM_HOME", dir.path());
    f(dir.path())
}

/// Set `$CSM_HOME` to `val` for `f`, restoring it afterwards. Tests using this
/// must be `#[serial]`.
pub(crate) fn with_csm_home_val<R>(val: &str, f: impl FnOnce() -> R) -> R {
    let _guard = CsmHomeGuard::new();
    std::env::set_var("CSM_HOME", val);
    f()
}
