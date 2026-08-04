//! Shared test helpers for integration tests that need private env / fs state.
//!
//! Integration tests across modules mutate process-global env vars (`CSM_HOME`,
//! `HOME`, `CSM_SESSION`), so they must run `#[serial]` and share one set of
//! isolation helpers. `pub(crate)` here is the legitimate cross-module
//! test-helper case (not exposing a unit under test; an inline `mod tests`
//! already reaches parent-private items via `use super::*`).

use std::path::Path;
use tempfile::TempDir;

use crate::store;
use crate::workspace;

/// Save one env var and restore it on drop - panic-safe.
struct EnvGuard {
    key: &'static str,
    prior: Option<String>,
}

impl EnvGuard {
    fn new(key: &'static str) -> Self {
        EnvGuard {
            key,
            prior: std::env::var(key).ok(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Point `$CSM_HOME` at a fresh temp dir for `f`, restoring it afterwards.
/// Tests using this must be `#[serial]`.
pub(crate) fn with_csm_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let _g = EnvGuard::new("CSM_HOME");
    let dir = TempDir::new().unwrap();
    std::env::set_var("CSM_HOME", dir.path());
    f(dir.path())
}

/// Set `$CSM_HOME` to `val` for `f`, restoring it afterwards. `#[serial]`.
pub(crate) fn with_csm_home_val<R>(val: &str, f: impl FnOnce() -> R) -> R {
    let _g = EnvGuard::new("CSM_HOME");
    std::env::set_var("CSM_HOME", val);
    f()
}

/// Point `$HOME` at a fresh temp dir for `f`, restoring it afterwards. Use for
/// tests that touch `~/.claude` / `~/.pi` (which key off `$HOME`). `#[serial]`.
pub(crate) fn with_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let _g = EnvGuard::new("HOME");
    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());
    f(dir.path())
}

/// Isolate both `$HOME` and `$CSM_HOME` to the same fresh temp dir for `f`
/// (e.g. `install_claude` writes `~/.claude` and renders `$CSM_HOME` into the
/// block). Builds on [`with_home`] (which owns the temp dir and `$HOME` guard),
/// pinning `$CSM_HOME` to the same dir. `#[serial]`.
pub(crate) fn with_isolated_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    with_home(|dir| {
        let _c = EnvGuard::new("CSM_HOME");
        std::env::set_var("CSM_HOME", dir);
        f(dir)
    })
}

/// Set `key` to `val` for `f`, restoring it afterwards. `#[serial]`.
pub(crate) fn with_env<R>(key: &'static str, val: &str, f: impl FnOnce() -> R) -> R {
    let _g = EnvGuard::new(key);
    std::env::set_var(key, val);
    f()
}

/// Unset `key` for `f`, restoring it afterwards. `#[serial]`.
pub(crate) fn without_env<R>(key: &'static str, f: impl FnOnce() -> R) -> R {
    let _g = EnvGuard::new(key);
    std::env::remove_var(key);
    f()
}

/// Create a session with a scaffolded workspace: `touch_session` (index entry)
/// then `ensure_workspace` (state.md / progress.md / scripts/INDEX.md /
/// notes/INDEX.md / tasks/INDEX.md). The shared core of "make a healthy
/// session" used by the `hook`, `gc`, and `doctor` integration tests.
/// `#[serial]` (mutates `$CSM_HOME`).
pub(crate) fn scaffold_session(name: &str) -> store::SessionMeta {
    let meta = store::touch_session(name, "/o").unwrap();
    workspace::ensure_workspace(name, &meta).unwrap();
    meta
}
