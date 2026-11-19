//! Session metadata and the on-disk index.

use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub origin_pwd: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registered_dirs: Vec<String>,
    pub created_at: String,
    pub last_access: String,
    pub pinned: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_sprint: bool,
}

impl SessionMeta {
    pub fn is_registered_in(&self, dir: &str) -> bool {
        self.origin_pwd == dir || self.registered_dirs.iter().any(|path| path == dir)
    }

    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    #[serde(default)]
    pub sessions: BTreeMap<String, SessionMeta>,
}

pub fn csm_home() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("CSM_HOME") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
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
    let data =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    if data.trim().is_empty() {
        return Ok(Index::default());
    }
    let idx: Index =
        serde_json::from_str(&data).with_context(|| format!("parsing {}", path.display()))?;
    Ok(idx)
}

/// Lock before loading the index. Keep the file open through the write.
fn load_index_for_update() -> Result<(File, Index)> {
    let home = csm_home()?;
    std::fs::create_dir_all(&home)?;
    let path = home.join("index.lock");
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    lock.lock()
        .with_context(|| format!("locking {}", path.display()))?;
    Ok((lock, load_index()?))
}

fn save_index(idx: &Index) -> Result<()> {
    let path = index_path()?;
    let mut temp = tempfile::NamedTempFile::new_in(csm_home()?)?;
    serde_json::to_writer_pretty(&mut temp, idx)?;
    temp.flush()?;
    temp.persist(&path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

pub fn require_session(name: &str) -> Result<SessionMeta> {
    let idx = load_index()?;
    idx.sessions
        .get(name)
        .cloned()
        .with_context(|| format!("no csm session named {:?}", name))
}

pub fn touch_session(name: &str, origin_pwd: &str) -> Result<SessionMeta> {
    Ok(touch_session_reviving(name, origin_pwd)?.0)
}

pub fn touch_session_reviving(name: &str, origin_pwd: &str) -> Result<(SessionMeta, bool)> {
    let (_lock, mut idx) = load_index_for_update()?;
    let meta = idx
        .sessions
        .entry(name.to_string())
        .or_insert_with(|| SessionMeta {
            origin_pwd: origin_pwd.to_string(),
            registered_dirs: Vec::new(),
            created_at: now_iso(),
            last_access: now_iso(),
            pinned: false,
            archived_at: None,
            in_sprint: false,
        });
    let revived = meta.archived_at.take().is_some();
    meta.last_access = now_iso();
    let clone = meta.clone();
    save_index(&idx)?;
    Ok((clone, revived))
}

/// Revalidate a picker selection and hold the lock through agent startup.
pub fn start_picked_session<T>(
    name: &str,
    created_at: &str,
    register_dir: Option<&str>,
    start: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let (_lock, mut idx) = load_index_for_update()?;
    let meta = idx
        .sessions
        .get_mut(name)
        .filter(|meta| !meta.is_archived() && meta.created_at == created_at)
        .with_context(|| {
            format!("session {name:?} is no longer available. Reopen the picker to choose again")
        })?;
    if let Some(dir) = register_dir.filter(|dir| !meta.is_registered_in(dir)) {
        meta.registered_dirs.push(dir.to_string());
    }
    meta.last_access = now_iso();
    crate::workspace::ensure_workspace(name, meta)?;
    save_index(&idx)?;
    start()
}

/// Refresh access time without creating a session or reviving an archived one.
pub fn touch_if_exists(name: &str) -> Result<Option<SessionMeta>> {
    let (_lock, mut idx) = load_index_for_update()?;
    let meta = match idx.sessions.get_mut(name) {
        Some(m) => m,
        None => return Ok(None),
    };
    meta.last_access = now_iso();
    let clone = meta.clone();
    save_index(&idx)?;
    Ok(Some(clone))
}

pub fn set_pinned(name: &str, pinned: bool) -> Result<()> {
    let (_lock, mut idx) = load_index_for_update()?;
    let meta = idx
        .sessions
        .get_mut(name)
        .with_context(|| format!("no csm session named {:?}", name))?;
    meta.pinned = pinned;
    save_index(&idx)
}

pub fn set_archived(name: &str, archived: bool) -> Result<bool> {
    let (_lock, mut idx) = load_index_for_update()?;
    let meta = idx
        .sessions
        .get_mut(name)
        .with_context(|| format!("no csm session named {:?}", name))?;
    let changed = archived != meta.is_archived();
    if changed {
        meta.archived_at = archived.then(now_iso);
        save_index(&idx)?;
    }
    Ok(changed)
}

pub fn set_in_sprint(name: &str, in_sprint: bool) -> Result<()> {
    let (_lock, mut idx) = load_index_for_update()?;
    let meta = idx
        .sessions
        .get_mut(name)
        .with_context(|| format!("no csm session named {:?}", name))?;
    if in_sprint && meta.is_archived() {
        anyhow::bail!(
            "session {:?} is archived; unarchive it before adding it to the sprint",
            name
        );
    }
    if meta.in_sprint != in_sprint {
        meta.in_sprint = in_sprint;
        save_index(&idx)?;
    }
    Ok(())
}

/// Rename and re-home a session. Equal names only update its origin and access time.
pub fn rename_session(old: &str, new: &str, new_origin_pwd: &str) -> Result<()> {
    let (_lock, mut idx) = load_index_for_update()?;

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

    // Move the workspace before updating the index so a failed move leaves it intact.
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

pub fn delete_session(name: &str) -> Result<()> {
    let (_lock, mut idx) = load_index_for_update()?;
    if !idx.sessions.contains_key(name) {
        anyhow::bail!("no csm session named {:?}", name);
    }
    idx.sessions.remove(name);
    save_index(&idx)?;
    let dir = session_dir(name)?;
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
pub fn set_last_access(name: &str, timestamp: String) -> Result<()> {
    let (_lock, mut idx) = load_index_for_update()?;
    idx.sessions.get_mut(name).unwrap().last_access = timestamp;
    save_index(&idx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{with_csm_home, with_csm_home_val};
    use crate::workspace::ensure_workspace;
    use serial_test::serial;

    #[test]
    #[serial]
    fn csm_home_uses_csm_home_when_set() {
        with_csm_home(|dir| {
            assert_eq!(csm_home().unwrap(), dir);
        });
    }

    #[test]
    #[serial]
    fn csm_home_falls_back_when_csm_home_empty() {
        with_csm_home_val("", || {
            let home = csm_home().unwrap();
            assert!(
                home.ends_with(".csm"),
                "empty CSM_HOME should fall back to $HOME/.csm, got {home:?}"
            );
        });
    }

    #[test]
    #[serial]
    fn touch_require_delete_round_trip_is_isolated() {
        with_csm_home(|_dir| {
            let meta = touch_session("smoke", "/tmp/origin").unwrap();
            assert_eq!(meta.origin_pwd, "/tmp/origin");
            assert!(!meta.pinned);
            assert!(require_session("smoke").is_ok());
            delete_session("smoke").unwrap();
            assert!(require_session("smoke").is_err());
        });
    }

    #[test]
    fn parse_time_valid_preserves_instant() {
        let dt = parse_time("2026-07-30T15:04:56+00:00").unwrap();
        assert_eq!(dt.timestamp(), 1785423896);
    }

    #[test]
    fn parse_time_invalid_errors() {
        assert!(parse_time("not-a-timestamp").is_err());
    }

    #[test]
    fn format_ts_passes_through_unparseable() {
        let raw = "not-a-timestamp";
        assert_eq!(format_ts(raw), raw);
    }

    #[test]
    fn format_ts_reformats_valid_input() {
        let out = format_ts("2026-07-30T15:04:56+00:00");
        assert_ne!(out, "2026-07-30T15:04:56+00:00");
        assert_eq!(out.len(), 16);
        let b = out.as_bytes();
        assert_eq!(b[4], b'-');
        assert_eq!(b[7], b'-');
        assert_eq!(b[10], b' ');
        assert_eq!(b[13], b':');
    }

    fn state_marker(name: &str) -> String {
        std::fs::read_to_string(session_dir(name).unwrap().join("state.md")).unwrap()
    }

    #[test]
    #[serial]
    fn touch_session_never_overwrites_origin_pwd() {
        with_csm_home(|_dir| {
            let meta = touch_session("a", "/orig").unwrap();
            assert_eq!(meta.origin_pwd, "/orig");
            assert!(!meta.pinned);

            let _ = touch_session("a", "/different").unwrap();
            assert_eq!(require_session("a").unwrap().origin_pwd, "/orig");
        });
    }

    #[test]
    #[serial]
    fn touch_if_exists_returns_none_for_unknown_and_does_not_create() {
        with_csm_home(|_dir| {
            assert!(touch_if_exists("ghost").unwrap().is_none());
            assert!(require_session("ghost").is_err());
        });
    }

    #[test]
    #[serial]
    fn touch_if_exists_refreshes_known_session() {
        with_csm_home(|_dir| {
            touch_session("a", "/orig").unwrap();
            let meta = touch_if_exists("a").unwrap().expect("known session");
            assert_eq!(meta.origin_pwd, "/orig");
        });
    }

    #[test]
    #[serial]
    fn set_pinned_toggles_and_errors_unknown() {
        with_csm_home(|_dir| {
            touch_session("a", "/o").unwrap();
            set_pinned("a", true).unwrap();
            assert!(require_session("a").unwrap().pinned);
            set_pinned("a", false).unwrap();
            assert!(!require_session("a").unwrap().pinned);
            assert!(set_pinned("ghost", true).is_err());
        });
    }

    #[test]
    fn session_meta_archived_at_serializes_sparsely() {
        let active = SessionMeta {
            origin_pwd: "/o".into(),
            registered_dirs: Vec::new(),
            created_at: "2026-10-01T00:00:00+00:00".into(),
            last_access: "2026-10-01T00:00:00+00:00".into(),
            pinned: false,
            archived_at: None,
            in_sprint: false,
        };
        let json = serde_json::to_string(&active).unwrap();
        assert!(!json.contains("archived_at"), "{json}");

        let archived = SessionMeta {
            archived_at: Some("2026-10-21T09:14:00+00:00".into()),
            ..active
        };
        let json = serde_json::to_string(&archived).unwrap();
        assert!(json.contains("archived_at"), "{json}");

        let minimal = r#"{"origin_pwd":"/o","created_at":"t","last_access":"t","pinned":false}"#;
        let parsed: SessionMeta = serde_json::from_str(minimal).unwrap();
        assert!(parsed.archived_at.is_none());
        assert!(parsed.is_registered_in("/o"));
        assert!(!parsed.is_registered_in("/other"));
    }

    #[test]
    #[serial]
    fn set_archived_is_idempotent_and_errors_unknown() {
        with_csm_home(|_dir| {
            touch_session("a", "/o").unwrap();
            assert!(set_archived("a", true).unwrap());
            let first = require_session("a").unwrap().archived_at.unwrap();
            assert!(!set_archived("a", true).unwrap());
            assert_eq!(require_session("a").unwrap().archived_at.unwrap(), first);

            assert!(set_archived("a", false).unwrap());
            assert!(require_session("a").unwrap().archived_at.is_none());
            assert!(!set_archived("a", false).unwrap(), "already active");

            assert!(set_archived("ghost", true).is_err());
            assert!(set_archived("ghost", false).is_err());
        });
    }

    #[test]
    #[serial]
    fn touch_session_revives_archived_and_reports_it() {
        with_csm_home(|_dir| {
            touch_session("a", "/o").unwrap();
            set_archived("a", true).unwrap();

            let (meta, revived) = touch_session_reviving("a", "/o").unwrap();
            assert!(revived, "touching an archived session revives it");
            assert!(meta.archived_at.is_none());
            assert!(require_session("a").unwrap().archived_at.is_none());

            let (_, revived) = touch_session_reviving("a", "/o").unwrap();
            assert!(!revived, "an active session has nothing to revive");
            let (_, revived) = touch_session_reviving("new", "/o").unwrap();
            assert!(!revived, "a fresh session was never archived");
        });
    }

    #[test]
    #[serial]
    fn touch_if_exists_preserves_archived_at() {
        with_csm_home(|_dir| {
            touch_session("a", "/o").unwrap();
            set_archived("a", true).unwrap();
            let meta = touch_if_exists("a").unwrap().expect("known session");
            assert!(meta.archived_at.is_some());
            assert!(require_session("a").unwrap().archived_at.is_some());
        });
    }

    #[test]
    #[serial]
    fn picker_rejects_archived_renamed_and_replaced_sessions() {
        with_csm_home(|_dir| {
            let chosen = touch_session("a", "/origin").unwrap();
            set_archived("a", true).unwrap();
            let start = || -> Result<()> { panic!("a stale session must not launch") };
            assert!(start_picked_session("a", &chosen.created_at, Some("/other"), start).is_err());
            let archived = require_session("a").unwrap();
            assert!(archived.is_archived());
            assert!(archived.registered_dirs.is_empty());
            assert_eq!(archived.last_access, chosen.last_access);
            assert!(!session_dir("a").unwrap().exists());

            set_archived("a", false).unwrap();
            rename_session("a", "b", "/origin").unwrap();
            assert!(start_picked_session("a", &chosen.created_at, None, start).is_err());
            assert!(require_session("a").is_err());
            assert!(!session_dir("a").unwrap().exists());

            let replacement = touch_session("a", "/different").unwrap();
            assert_ne!(replacement.created_at, chosen.created_at);
            assert!(start_picked_session("a", &chosen.created_at, None, start).is_err());
            assert!(!session_dir("a").unwrap().exists());
        });
    }

    #[test]
    #[serial]
    fn concurrent_picker_registrations_keep_both_directories() {
        with_csm_home(|_dir| {
            let chosen = touch_session("a", "/origin").unwrap();
            let start = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                for dir in ["/first", "/second"] {
                    let start = &start;
                    let created_at = &chosen.created_at;
                    scope.spawn(move || {
                        start.wait();
                        start_picked_session("a", created_at, Some(dir), || Ok(())).unwrap();
                    });
                }
            });
            let meta = require_session("a").unwrap();
            for dir in ["/origin", "/first", "/second"] {
                assert!(meta.is_registered_in(dir), "missing directory: {dir}");
            }
        });
    }

    #[test]
    #[serial]
    fn rename_old_eq_new_only_re_homes() {
        with_csm_home(|_dir| {
            let meta = touch_session("a", "/orig").unwrap();
            ensure_workspace("a", &meta).unwrap();
            let dir_before = session_dir("a").unwrap();
            let marker = state_marker("a");

            rename_session("a", "a", "/new").unwrap();
            assert_eq!(require_session("a").unwrap().origin_pwd, "/new");
            assert_eq!(session_dir("a").unwrap(), dir_before);
            assert_eq!(state_marker("a"), marker);
        });
    }

    #[test]
    #[serial]
    fn rename_bails_when_target_exists() {
        with_csm_home(|_dir| {
            touch_session("a", "/oa").unwrap();
            touch_session("b", "/ob").unwrap();
            assert!(rename_session("a", "b", "/x").is_err());
            assert_eq!(require_session("a").unwrap().origin_pwd, "/oa");
            assert_eq!(require_session("b").unwrap().origin_pwd, "/ob");
        });
    }

    #[test]
    #[serial]
    fn rename_moves_dir_and_rekeys_index() {
        with_csm_home(|_dir| {
            let meta = touch_session("a", "/oa").unwrap();
            ensure_workspace("a", &meta).unwrap();
            assert!(session_dir("a").unwrap().exists());

            rename_session("a", "c", "/oc").unwrap();
            assert_eq!(require_session("c").unwrap().origin_pwd, "/oc");
            assert!(require_session("a").is_err());
            assert!(session_dir("c").unwrap().exists());
            assert!(!session_dir("a").unwrap().exists());
        });
    }

    #[test]
    #[serial]
    fn delete_removes_dir_and_index_entry() {
        with_csm_home(|_dir| {
            let meta = touch_session("a", "/oa").unwrap();
            ensure_workspace("a", &meta).unwrap();
            assert!(session_dir("a").unwrap().exists());

            delete_session("a").unwrap();
            assert!(require_session("a").is_err());
            assert!(!session_dir("a").unwrap().exists());
        });
    }
}
