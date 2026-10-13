//! Hermetic sandbox construction: a temp root holding fresh HOME / CSM_HOME /
//! CODEX_HOME / XDG dirs, the fixture repo + local bare remote, and the
//! deployed csm wiring. Everything the agent can see or write lives under the
//! sandbox root.

use crate::scenario::{Arm, Scenario};
use anyhow::{bail, Context, Result};
use csm::inject;
use csm::prompt::{csm_block, CSM_MARK_BEGIN, CSM_MARK_END};
use csm::skills;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The sandbox root layout. Paths are stable per run; the root is unique per
/// run (`$TMPDIR/csm-eval-<timestamp>-<pid>`).
pub struct Sandbox {
    pub root: PathBuf,
    pub home: PathBuf,
    pub csm_home: PathBuf,
    pub codex_home: PathBuf,
    pub xdg_config: PathBuf,
    pub xdg_cache: PathBuf,
    pub xdg_data: PathBuf,
    pub tmp: PathBuf,
    /// The fixture repo - the agent's cwd for every turn.
    pub repo: PathBuf,
    /// The local bare remote (`git push` succeeds harmlessly, visibly).
    pub remote: PathBuf,
}

impl Sandbox {
    pub fn create() -> Result<Sandbox> {
        // Timestamp + a process-wide counter: two sandboxes created in the
        // same second from the same process (parallel tests, rapid runs)
        // must never share a root.
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        // Short root under /tmp: claude derives its per-uid staging dir from
        // $CLAUDE_CODE_TMPDIR (see apply_env) and AF_UNIX socket paths cap at
        // ~104 bytes, so /var/folders roots are too long for the override to
        // stick. /tmp is the standard temp dir on both platforms.
        let base = if std::path::Path::new("/tmp").is_dir() {
            std::path::PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        };
        let root = base.join(format!(
            "csm-eval-{}-{}-{}",
            chrono::Local::now().format("%m%d-%H%M%S"),
            seq,
            std::process::id()
        ));
        let sb = Sandbox {
            home: root.join("home"),
            csm_home: root.join("csm"),
            codex_home: root.join("codex"),
            xdg_config: root.join("xdg-config"),
            xdg_cache: root.join("xdg-cache"),
            xdg_data: root.join("xdg-data"),
            tmp: root.join("tmp"),
            repo: root.join("repo"),
            remote: root.join("remote.git"),
            root: root.clone(),
        };
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
        }
        for dir in [
            &sb.home,
            &sb.csm_home,
            &sb.codex_home,
            &sb.xdg_config,
            &sb.xdg_cache,
            &sb.xdg_data,
            &sb.tmp,
            &sb.repo,
        ] {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        Ok(sb)
    }

    /// Remove the sandbox (unless `--keep-sandbox`).
    pub fn remove(&self) -> Result<()> {
        std::fs::remove_dir_all(&self.root)
            .with_context(|| format!("removing {}", self.root.display()))
    }
}

/// The `csm` binary next to this binary (both targets land in the same cargo
/// output dir; under `cargo test` the test binary sits one level deeper in
/// `deps/`, so walk up a couple of levels), so the sandbox runs the exact
/// build under test.
pub fn sibling_csm() -> Result<PathBuf> {
    let exe = std::env::current_exe().context("locating the csm-eval binary")?;
    let mut dir = exe
        .parent()
        .context("locating the csm-eval binary's directory")?
        .to_path_buf();
    for _ in 0..3 {
        let csm = dir.join("csm");
        if csm.exists() {
            return Ok(csm);
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    bail!(
        "the `csm` binary was not found near {}; build both with `cargo build`",
        exe.display()
    )
}

/// Apply the curated sandbox environment to `cmd`: a scratch env built from
/// nothing - HOME/CSM_HOME/CODEX_HOME/XDG_*/TMPDIR point into the sandbox,
/// `$CSM_SESSION` binds the fixture session, and only the model-provider
/// variables are passed through from the parent (no `GH_*`/`GITHUB_*`, so
/// `gh` stays unauthenticated and PR attempts fail observably).
pub fn apply_env(cmd: &mut Command, sb: &Sandbox, session_name: &str) {
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        let key = k.to_string_lossy().to_string();
        let keep = key.starts_with("ANTHROPIC_") || key == "OPENAI_API_KEY";
        if keep {
            cmd.env(k, v);
        }
    }
    cmd.env("HOME", &sb.home);
    cmd.env("TMPDIR", &sb.tmp);
    cmd.env("CSM_HOME", &sb.csm_home);
    cmd.env("CODEX_HOME", &sb.codex_home);
    cmd.env("XDG_CONFIG_HOME", &sb.xdg_config);
    cmd.env("XDG_CACHE_HOME", &sb.xdg_cache);
    cmd.env("XDG_DATA_HOME", &sb.xdg_data);
    cmd.env("CSM_SESSION", session_name);
    // claude stages shell snapshots under `$CLAUDE_CODE_TMPDIR/claude-<uid>`;
    // without this it defaults to /tmp/claude-<uid>, outside the sandbox (and
    // seatbelt-denied). Must stay short: its socket paths cap at ~104 bytes.
    cmd.env("CLAUDE_CODE_TMPDIR", &sb.tmp);
    cmd.env("LANG", "en_US.UTF-8");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    // `csm hook` (the SessionStart command) must resolve to the build under
    // test, so the sandbox PATH puts the cargo output dir first.
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    if let Some(dir) = exe_dir {
        let inherited = std::env::var("PATH").unwrap_or_default();
        cmd.env("PATH", format!("{}:{inherited}", dir.display()));
    }
}

/// Run `csm init` under the sandbox env so the real deploy code path (inject /
/// hook wiring / skills) writes the working-tree block into the sandbox
/// config. The `no-block` arm skips wiring entirely.
pub fn deploy_wiring(sb: &Sandbox, arm: Arm, session_name: &str) -> Result<()> {
    if arm == Arm::NoBlock {
        return Ok(());
    }
    let csm_bin = sibling_csm()?;
    let mut cmd = Command::new(&csm_bin);
    cmd.arg("init");
    apply_env(&mut cmd, sb, session_name);
    let out = cmd
        .output()
        .with_context(|| format!("running {}", csm_bin.display()))?;
    if !out.status.success() {
        bail!(
            "csm init failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Post-deploy self-check (working-tree arm): the sandbox harness config must
/// carry the working-tree [csm] block, the SessionStart hook, and the shipped
/// skills. Returns the block's sha256 over the marker-bounded region.
pub fn deploy_check(sb: &Sandbox, arm: Arm) -> Result<Option<String>> {
    if arm == Arm::NoBlock {
        return Ok(None);
    }
    let md_path = sb.home.join(".claude").join("CLAUDE.md");
    let md = std::fs::read_to_string(&md_path)
        .with_context(|| format!("reading {}", md_path.display()))?;
    let begin = md
        .find(CSM_MARK_BEGIN)
        .context("sandbox CLAUDE.md is missing the [csm] block begin marker")?;
    let end = md
        .find(CSM_MARK_END)
        .context("sandbox CLAUDE.md is missing the [csm] block end marker")?;
    anyhow::ensure!(end >= begin, "[csm] block markers out of order");
    let block = &md[begin..end + CSM_MARK_END.len()];

    // The deployed block must be the working-tree block (same home rendering).
    anyhow::ensure!(
        block == csm_block(&sb.csm_home.display().to_string()),
        "deployed [csm] block differs from the working-tree block"
    );

    let settings_path = sb.home.join(".claude").join("settings.json");
    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&settings_path)
            .with_context(|| format!("reading {}", settings_path.display()))?,
    )
    .context("sandbox settings.json is not valid JSON")?;
    anyhow::ensure!(
        inject::sessionstart_hook_present(&settings),
        "sandbox settings.json is missing the csm SessionStart hook"
    );

    for skill in skills::SKILLS {
        let path = sb
            .home
            .join(".claude")
            .join("skills")
            .join(skill.id)
            .join("SKILL.md");
        anyhow::ensure!(
            path.exists(),
            "skill {} not deployed to the sandbox",
            skill.id
        );
    }

    Ok(Some(sha256_hex(block.as_bytes())?))
}

/// Restore `$CSM_HOME` on drop, panic-safe (same pattern as the crate's test
/// helpers: the fixture session is scaffolded through the real store/workspace
/// functions, which read `$CSM_HOME` from the process env).
struct CsmHomeGuard {
    prior: Option<String>,
}

impl Drop for CsmHomeGuard {
    fn drop(&mut self) {
        match &self.prior {
            Some(v) => std::env::set_var("CSM_HOME", v),
            None => std::env::remove_var("CSM_HOME"),
        }
    }
}

/// Create the fixture session through the real product path (`touch_session` +
/// `ensure_workspace`), then overwrite `state.md` / the board with the
/// scenario's fixture content and write the extra task files.
pub fn scaffold_fixture_session(sb: &Sandbox, scenario: &Scenario) -> Result<()> {
    let _guard = CsmHomeGuard {
        prior: std::env::var("CSM_HOME").ok(),
    };
    std::env::set_var("CSM_HOME", &sb.csm_home);

    let name = &scenario.session.name;
    let meta = csm::store::touch_session(name, &sb.repo.display().to_string())
        .context("creating the fixture session")?;
    csm::workspace::ensure_workspace(name, &meta).context("scaffolding the fixture workspace")?;
    let dir = csm::store::session_dir(name)?;
    std::fs::write(dir.join("state.md"), &scenario.session.state_md)
        .context("writing fixture state.md")?;
    std::fs::write(
        dir.join("tasks").join("INDEX.md"),
        &scenario.session.tasks_index_md,
    )
    .context("writing fixture tasks/INDEX.md")?;
    for (rel, content) in &scenario.session.task_files {
        anyhow::ensure!(
            !rel.starts_with('/') && !rel.split('/').any(|seg| seg == ".."),
            "fixture task file path escapes the tasks dir: {rel:?}"
        );
        let path = dir.join("tasks").join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    Ok(())
}

/// Seed the fixture repo: fixed files, one initial commit under a fixed
/// repo-local identity, and a local bare remote already pushed to.
pub fn scaffold_fixture_repo(sb: &Sandbox, scenario: &Scenario) -> Result<()> {
    git(&sb.repo, &["init", "-b", "main"])?;
    // Fixed identity so commits carry no machine-local data.
    git(&sb.repo, &["config", "user.name", "csm-eval"])?;
    git(
        &sb.repo,
        &["config", "user.email", "csm-eval@fixture.invalid"],
    )?;
    git(&sb.repo, &["config", "commit.gpgsign", "false"])?;
    for (rel, content) in &scenario.repo.files {
        let path = sb.repo.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    git(&sb.repo, &["add", "-A"])?;
    git(&sb.repo, &["commit", "-m", &scenario.repo.initial_commit])?;
    std::process::Command::new("git")
        .arg("init")
        .arg("--bare")
        .arg("-b")
        .arg("main")
        .arg(&sb.remote)
        .output()
        .context("creating the bare remote")?;
    git(
        &sb.repo,
        &["remote", "add", "origin", &sb.remote.display().to_string()],
    )?;
    git(&sb.repo, &["push", "-u", "origin", "main"])?;
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Write a seatbelt profile confining writes to the sandbox root (macOS). The
/// agent process tree runs under it; a write outside the sandbox fails with
/// EPERM instead of landing on the real home. Returns `None` when
/// `sandbox-exec` is unavailable (non-macOS), letting the post-run filesystem
/// assertion stand alone.
pub fn write_seatbelt_profile(sb: &Sandbox) -> Result<Option<PathBuf>> {
    let available = Command::new("sandbox-exec")
        .arg("help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok();
    if !available {
        return Ok(None);
    }
    let path = sb.root.join("sandbox.sb");
    // Seatbelt evaluates real paths: /tmp symlinks to /private/tmp (and
    // $TMPDIR-style roots live under /var/folders → /private/var), so the
    // allow rule must name the canonical root.
    let root = std::fs::canonicalize(&sb.root).unwrap_or_else(|_| sb.root.clone());
    let profile = format!(
        "(version 1)\n\
         (allow default)\n\
         (deny file-write*)\n\
         (allow file-write* (subpath \"{}\") (subpath \"/dev\"))\n",
        root.display()
    );
    std::fs::write(&path, profile)?;
    Ok(Some(path))
}

/// Route `cmd` through `sandbox-exec` when a profile is present, preserving
/// the already-configured env and cwd.
pub fn wrap_seatbelt(cmd: &mut Command, profile: &Option<PathBuf>) {
    let Some(profile) = profile else {
        return;
    };
    let program = cmd.get_program().to_os_string();
    let args: Vec<std::ffi::OsString> = cmd.get_args().map(|a| a.to_os_string()).collect();
    // The curated env must survive the wrap (std has no current_dir getter, so
    // callers set cwd after wrapping).
    let envs: Vec<(std::ffi::OsString, std::ffi::OsString)> = cmd
        .get_envs()
        .filter_map(|(k, v)| v.map(|v| (k.to_os_string(), v.to_os_string())))
        .collect();
    let mut wrapped = Command::new("sandbox-exec");
    wrapped.arg("-f").arg(profile).arg(&program);
    for a in args {
        wrapped.arg(a);
    }
    for (k, v) in envs {
        wrapped.env(k, v);
    }
    *cmd = wrapped;
}

/// sha256 over bytes, via the platform digest tool (no crypto dependency in
/// the shared crate: the product binary links what this crate links).
pub fn sha256_hex(data: &[u8]) -> Result<String> {
    for (tool, args) in [("shasum", vec!["-a", "256"]), ("sha256sum", vec![])] {
        let Ok(mut child) = Command::new(tool)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
        else {
            continue;
        };
        child
            .stdin
            .take()
            .context("digest tool stdin")?
            .write_all(data)?;
        let out = child.wait_with_output()?;
        if out.status.success() {
            let line = String::from_utf8_lossy(&out.stdout);
            return Ok(line
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string());
        }
    }
    bail!("no sha256 tool available (shasum/sha256sum)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::new_uuid;
    use crate::scenario::{FixtureRepo, FixtureSession, Scenario};
    use serial_test::serial;
    use std::collections::BTreeMap;

    /// Lookup one env var on a built Command (std exposes get_envs, not get_env).
    fn env_of(cmd: &Command, key: &str) -> Option<String> {
        let want = std::ffi::OsStr::new(key);
        cmd.get_envs()
            .find(|(k, _)| *k == want)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().to_string()))
    }

    fn minimal_scenario(name: &str) -> Scenario {
        let mut files = BTreeMap::new();
        files.insert(String::from("README.md"), String::from("# demo\n"));
        let mut task_files = BTreeMap::new();
        task_files.insert(
            String::from("001-demo.md"),
            String::from("# 001 demo\n\n## Scope\n\nDemo task.\n"),
        );
        Scenario {
            name: name.to_string(),
            behavior: String::from("behavior"),
            description: String::from("test fixture"),
            session: FixtureSession {
                name: String::from("eval-fixture"),
                state_md: String::from("# eval-fixture - state\n\n## Context\nTest.\n"),
                tasks_index_md: String::from(
                    "# eval-fixture - tasks board\n\n## Open\n- 001 demo - demo task\n",
                ),
                task_files,
            },
            repo: FixtureRepo {
                files,
                initial_commit: String::from("seed"),
            },
            script: vec![String::from("go")],
        }
    }

    #[test]
    fn apply_env_builds_scratch_env_with_provider_passthrough() {
        let sb = Sandbox::create().unwrap();
        let mut cmd = Command::new("true");
        // A gh token in the parent must not leak into the sandbox env.
        std::env::set_var("GH_TOKEN", "leak-me-not");
        std::env::set_var("ANTHROPIC_AUTH_TOKEN", "pass-me-through");
        apply_env(&mut cmd, &sb, "eval-fixture");
        std::env::remove_var("GH_TOKEN");
        std::env::remove_var("ANTHROPIC_AUTH_TOKEN");

        assert_eq!(env_of(&cmd, "HOME"), Some(sb.home.display().to_string()));
        assert_eq!(env_of(&cmd, "TMPDIR"), Some(sb.tmp.display().to_string()));
        assert_eq!(
            env_of(&cmd, "CSM_HOME"),
            Some(sb.csm_home.display().to_string())
        );
        assert_eq!(
            env_of(&cmd, "CODEX_HOME"),
            Some(sb.codex_home.display().to_string())
        );
        assert_eq!(
            env_of(&cmd, "CSM_SESSION"),
            Some("eval-fixture".to_string())
        );
        assert_eq!(
            env_of(&cmd, "ANTHROPIC_AUTH_TOKEN"),
            Some("pass-me-through".to_string())
        );
        assert_eq!(env_of(&cmd, "GH_TOKEN"), None);
        // The sandbox PATH must lead with the cargo output dir so `csm hook`
        // resolves to the build under test.
        let path = env_of(&cmd, "PATH").unwrap();
        let exe_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(
            path.starts_with(&format!("{}:", exe_dir.display())),
            "PATH does not lead with the build dir: {path}"
        );
        let _ = sb.remove();
    }

    #[test]
    fn sha256_hex_matches_known_vector() {
        // sha256("abc")
        assert_eq!(
            sha256_hex(b"abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn new_uuid_is_canonical_v4() {
        let uuid = new_uuid().unwrap();
        let parts: Vec<&str> = uuid.split('-').collect();
        assert_eq!(parts.len(), 5);
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert!(uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-'));
        assert!(uuid.split('-').nth(2).unwrap().starts_with('4'));
    }

    #[test]
    #[serial]
    fn sandbox_deploys_wiring_and_scaffolds_fixture() {
        // Exercises the real product path end to end (local only): `csm init`
        // into the sandbox, the deploy self-check, the fixture session through
        // store/workspace, and the fixture repo with its bare remote.
        let scenario = minimal_scenario("sandbox-it");
        let sb = Sandbox::create().unwrap();
        deploy_wiring(&sb, Arm::WorkingTree, &scenario.session.name).unwrap();

        let block_sha = deploy_check(&sb, Arm::WorkingTree).unwrap().unwrap();
        assert_eq!(block_sha.len(), 64);
        let md = std::fs::read_to_string(sb.home.join(".claude").join("CLAUDE.md")).unwrap();
        assert!(md.contains(CSM_MARK_BEGIN) && md.contains(CSM_MARK_END));
        // The block is rendered against the sandbox CSM_HOME (path dynamization).
        assert!(md.contains(&sb.csm_home.display().to_string()));

        scaffold_fixture_session(&sb, &scenario).unwrap();
        let session_dir = sb.csm_home.join("sessions").join("eval-fixture");
        let state = std::fs::read_to_string(session_dir.join("state.md")).unwrap();
        assert!(state.contains("## Context"));
        let board = std::fs::read_to_string(session_dir.join("tasks").join("INDEX.md")).unwrap();
        assert!(board.contains("- 001 demo"));
        assert!(session_dir.join("tasks").join("001-demo.md").exists());
        // The scaffold's registry files survive the fixture overwrite.
        assert!(session_dir.join("notes").join("INDEX.md").exists());

        scaffold_fixture_repo(&sb, &scenario).unwrap();
        let out = Command::new("git")
            .arg("-C")
            .arg(&sb.repo)
            .args(["log", "--oneline"])
            .output()
            .unwrap();
        assert!(out.status.success());
        let log = String::from_utf8_lossy(&out.stdout);
        assert!(log.contains("seed"), "{log}");
        // The bare remote has the seed commit (push succeeded at seed time).
        let out = Command::new("git")
            .arg("-C")
            .arg(&sb.remote)
            .args(["rev-parse", "main"])
            .output()
            .unwrap();
        assert!(out.status.success());

        // The no-block arm deploys nothing and the check reports no block,
        // on a fresh sandbox (the arm means init never ran).
        let sb2 = Sandbox::create().unwrap();
        deploy_wiring(&sb2, Arm::NoBlock, "eval-fixture").unwrap();
        assert_eq!(deploy_check(&sb2, Arm::NoBlock).unwrap(), None);
        assert!(
            !sb2.home.join(".claude").join("CLAUDE.md").exists(),
            "no-block arm must not write any config"
        );
        sb2.remove().unwrap();

        sb.remove().unwrap();
    }

    #[test]
    fn seatbelt_profile_denies_writes_outside_sandbox() {
        // The profile is text we can inspect, not execute; assert its deny +
        // allow shape covers the sandbox root and /dev only.
        let sb = Sandbox::create().unwrap();
        let profile = write_seatbelt_profile(&sb).unwrap();
        if profile.is_none() {
            return; // no sandbox-exec on this platform
        }
        let text = std::fs::read_to_string(profile.as_ref().unwrap()).unwrap();
        let root = std::fs::canonicalize(&sb.root).unwrap_or_else(|_| sb.root.clone());
        assert!(text.contains("(deny file-write*)"));
        assert!(text.contains(&format!("(subpath \"{}\")", root.display())));
        assert!(text.contains("(subpath \"/dev\")"));
        sb.remove().unwrap();
    }
}
