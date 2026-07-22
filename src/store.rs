//! csm home layout, the kv index, and session metadata.
//!
//! Layout:
//!   ~/.csm/
//!     index.json          - kv: { sessions: { <name>: { origin_pwd, created_at, last_access, pinned } } }
//!     sessions/<name>/
//!       state.md
//!       progress.md
//!       scripts/INDEX.md
//!       scripts/...

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub origin_pwd: String,
    pub created_at: String,
    pub last_access: String,
    pub pinned: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionMeta>,
}

pub fn csm_home() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".csm"))
}

pub fn sessions_dir() -> Result<PathBuf> {
    Ok(csm_home()?.join("sessions"))
}

pub fn index_path() -> Result<PathBuf> {
    Ok(csm_home()?.join("index.json"))
}

pub fn session_dir(name: &str) -> Result<PathBuf> {
    Ok(sessions_dir()?.join(name))
}

pub fn now_iso() -> String {
    Local::now().to_rfc3339()
}

pub fn parse_time(s: &str) -> Result<DateTime<Local>> {
    Ok(DateTime::parse_from_rfc3339(s)?.with_timezone(&Local))
}

/// Format a stored timestamp (`last_access`/`created_at`) for display,
/// falling back to the raw string if it can't be parsed.
pub fn format_ts(ts: &str) -> String {
    parse_time(ts)
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|_| ts.to_string())
}

pub fn load_index() -> Result<Index> {
    let path = index_path()?;
    if !path.exists() {
        return Ok(Index::default());
    }
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    if data.trim().is_empty() {
        return Ok(Index::default());
    }
    let idx: Index = serde_json::from_str(&data)
        .with_context(|| format!("parsing {}", path.display()))?;
    Ok(idx)
}

pub fn save_index(idx: &Index) -> Result<()> {
    let path = index_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(idx)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Create the session if missing, refresh `last_access`, persist, return meta.
/// `origin_pwd` is only used at creation; it is never overwritten.
pub fn touch_session(name: &str, origin_pwd: &str) -> Result<SessionMeta> {
    let mut idx = load_index()?;
    let meta = idx.sessions.entry(name.to_string()).or_insert_with(|| SessionMeta {
        origin_pwd: origin_pwd.to_string(),
        created_at: now_iso(),
        last_access: now_iso(),
        pinned: false,
    });
    meta.last_access = now_iso();
    let clone = meta.clone();
    save_index(&idx)?;
    Ok(clone)
}

/// Like `touch_session` but never creates: returns `None` if the session does
/// not exist. Used by the SessionStart hook, which must not create sessions
/// for unknown `$CSM_SESSION` values.
pub fn touch_if_exists(name: &str) -> Result<Option<SessionMeta>> {
    let mut idx = load_index()?;
    let meta = match idx.sessions.get_mut(name) {
        Some(m) => m,
        None => return Ok(None),
    };
    meta.last_access = now_iso();
    let clone = meta.clone();
    save_index(&idx)?;
    Ok(Some(clone))
}

/// Update the pinned flag. Errors if the session does not exist.
pub fn set_pinned(name: &str, pinned: bool) -> Result<()> {
    let mut idx = load_index()?;
    let meta = idx
        .sessions
        .get_mut(name)
        .with_context(|| format!("no csm session named {:?}", name))?;
    meta.pinned = pinned;
    save_index(&idx)
}

/// Rename a session and re-point its `origin_pwd` to `new_origin_pwd`.
///
/// Moves the workspace dir (`<old>` -> `<new>`) and re-keys the index entry,
/// preserving `created_at` and `pinned`; refreshes `last_access`. Errors if
/// `old` doesn't exist or `new` already exists. If `old == new`, only re-points
/// `origin_pwd` (a pure re-home, no dir move).
pub fn rename_session(old: &str, new: &str, new_origin_pwd: &str) -> Result<()> {
    let mut idx = load_index()?;

    if old == new {
        let meta = idx
            .sessions
            .get_mut(old)
            .with_context(|| format!("no csm session named {:?}", old))?;
        meta.origin_pwd = new_origin_pwd.to_string();
        meta.last_access = now_iso();
        save_index(&idx)?;
        return Ok(());
    }

    if idx.sessions.contains_key(new) {
        anyhow::bail!("a session named {:?} already exists", new);
    }
    let meta = idx
        .sessions
        .get(old)
        .with_context(|| format!("no csm session named {:?}", old))?
        .clone();

    // Move the workspace dir before mutating the index (fail cleanly).
    let old_dir = session_dir(old)?;
    let new_dir = session_dir(new)?;
    if new_dir.exists() {
        anyhow::bail!("workspace dir already exists: {}", new_dir.display());
    }
    if old_dir.exists() {
        std::fs::rename(&old_dir, &new_dir)
            .with_context(|| format!("renaming {} -> {}", old_dir.display(), new_dir.display()))?;
    }

    idx.sessions.remove(old);
    let mut new_meta = meta;
    new_meta.origin_pwd = new_origin_pwd.to_string();
    new_meta.last_access = now_iso();
    idx.sessions.insert(new.to_string(), new_meta);
    save_index(&idx)?;
    Ok(())
}

/// Hard-delete a session: remove its workspace dir and index entry.
pub fn delete_session(name: &str) -> Result<()> {
    let mut idx = load_index()?;
    if !idx.sessions.contains_key(name) {
        anyhow::bail!("no csm session named {:?}", name);
    }
    idx.sessions.remove(name);
    save_index(&idx)?;
    let dir = session_dir(name)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .with_context(|| format!("removing {}", dir.display()))?;
    }
    Ok(())
}
