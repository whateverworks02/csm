//! Tests using these process-wide environment helpers must run with `#[serial]`.

use std::path::Path;
use tempfile::TempDir;

use crate::store;
use crate::workspace;

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

pub(crate) fn with_csm_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let _g = EnvGuard::new("CSM_HOME");
    let dir = TempDir::new().unwrap();
    std::env::set_var("CSM_HOME", dir.path());
    f(dir.path())
}

pub(crate) fn with_csm_home_val<R>(val: &str, f: impl FnOnce() -> R) -> R {
    let _g = EnvGuard::new("CSM_HOME");
    std::env::set_var("CSM_HOME", val);
    f()
}

pub(crate) fn with_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    let _g = EnvGuard::new("HOME");
    let dir = TempDir::new().unwrap();
    std::env::set_var("HOME", dir.path());
    f(dir.path())
}

/// Isolate home directories and clear XDG config to keep writes in the temporary directory.
pub(crate) fn with_isolated_home<R>(f: impl FnOnce(&Path) -> R) -> R {
    with_home(|dir| {
        let _c = EnvGuard::new("CSM_HOME");
        std::env::set_var("CSM_HOME", dir);
        let _x = EnvGuard::new("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_CONFIG_HOME");
        f(dir)
    })
}

pub(crate) fn with_env<R>(key: &'static str, val: &str, f: impl FnOnce() -> R) -> R {
    let _g = EnvGuard::new(key);
    std::env::set_var(key, val);
    f()
}

pub(crate) fn without_env<R>(key: &'static str, f: impl FnOnce() -> R) -> R {
    let _g = EnvGuard::new(key);
    std::env::remove_var(key);
    f()
}

pub(crate) fn scaffold_session(name: &str) -> store::SessionMeta {
    let meta = store::touch_session(name, "/o").unwrap();
    workspace::ensure_workspace(name, &meta).unwrap();
    meta
}

pub(crate) fn write_board(name: &str, body: &str) {
    std::fs::write(
        store::session_dir(name).unwrap().join("tasks/INDEX.md"),
        format!("# {name} - tasks board\n\n{body}"),
    )
    .unwrap();
}
