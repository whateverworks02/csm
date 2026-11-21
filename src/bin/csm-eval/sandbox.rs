use anyhow::{bail, ensure, Context, Result};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const SESSION: &str = "eval-fixture";

pub struct Sandbox {
    root: tempfile::TempDir,
    pub repo: PathBuf,
    pub session: PathBuf,
    pub codex_home: PathBuf,
}

impl Sandbox {
    pub fn create() -> Result<Self> {
        let root = tempfile::Builder::new().prefix("csm-eval-").tempdir()?;
        let path = root.path().canonicalize()?;
        let repo = path.join("repo");
        let codex_home = path.join("codex");
        for dir in [&repo, &codex_home, &path.join("home"), &path.join("tmp")] {
            fs::create_dir(dir)?;
        }
        Ok(Self {
            session: path.join("csm/sessions").join(SESSION),
            root,
            repo,
            codex_home,
        })
    }

    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        let root = self.repo.parent().expect("sandbox root");
        let mut command = Command::new(program);
        command
            .current_dir(&self.repo)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", root.join("home"))
            .env("TMPDIR", root.join("tmp"))
            .env("CODEX_HOME", &self.codex_home)
            .env("CSM_HOME", self.csm_home())
            .env("CSM_SESSION", SESSION)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        command
    }

    pub fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        let result = self.command("git").args(args).output()?;
        ensure!(
            result.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        );
        Ok(result.stdout)
    }

    pub fn read_only_command(&self) -> Command {
        let mut command = self.command("codex");
        command.args(["sandbox", "-P", ":read-only", "--"]);
        command
    }

    pub fn seed(
        &self,
        repo: &BTreeMap<String, String>,
        session: &BTreeMap<String, String>,
        other_sessions: &BTreeMap<String, BTreeMap<String, String>>,
    ) -> Result<()> {
        seed_files(&self.repo, repo)?;
        self.git(&["init", "--quiet", "-b", "main"])?;
        self.git(&["config", "user.name", "csm eval"])?;
        self.git(&["config", "user.email", "csm-eval@example.invalid"])?;
        self.git(&["add", "--all"])?;
        self.git(&[
            "commit",
            "--quiet",
            "--allow-empty",
            "-m",
            "Seed evaluation fixture",
        ])?;

        // Store and workspace creation use the same functions as a normal csm launch.
        let prior = std::env::var_os("CSM_HOME");
        std::env::set_var("CSM_HOME", self.csm_home());
        let scaffold = csm::store::touch_session(SESSION, &self.repo.display().to_string())
            .and_then(|meta| csm::workspace::ensure_workspace(SESSION, &meta));
        match prior {
            Some(home) => std::env::set_var("CSM_HOME", home),
            None => std::env::remove_var("CSM_HOME"),
        }
        scaffold?;
        for (name, files) in other_sessions {
            let path = relative_path(name)?;
            ensure!(
                path.file_name() == Some(OsStr::new(name)) && name != SESSION,
                "invalid sibling session {name:?}"
            );
            seed_files(&self.session.with_file_name(name), files)?;
        }
        seed_files(&self.session, session)
    }

    pub fn deploy(&self, csm: &Path, evidence: &Path) -> Result<()> {
        let path = std::env::join_paths(
            [csm.parent().context("csm binary directory")?.to_owned()]
                .into_iter()
                .chain(std::env::split_paths(
                    &std::env::var_os("PATH").unwrap_or_default(),
                )),
        )?;

        // csm init and the SessionStart hook must use the sibling build.
        let output = self.command(csm).env("PATH", &path).arg("init").output()?;
        ensure!(
            output.status.success(),
            "csm init: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let deployed = fs::read_to_string(self.codex_home.join("AGENTS.md"))?;
        let expected = csm::prompt::csm_block(&self.csm_home().display().to_string());
        ensure!(
            deployed.trim_end() == expected,
            "csm binary and runner disagree on the protocol; rebuild both"
        );
        fs::write(evidence.join("AGENTS.md"), deployed)?;

        // Only the known SessionStart hook is eligible to run unattended.
        let hooks = fs::read(self.codex_home.join("hooks.json"))?;
        ensure!(
            serde_json::from_slice::<serde_json::Value>(&hooks)?
                == serde_json::json!({
                    "hooks": { "SessionStart": [
                        { "matcher": "", "hooks": [{ "type": "command", "command": "csm hook" }] }
                    ] }
                }),
            "unexpected hooks in the disposable Codex config"
        );
        fs::write(evidence.join("hooks.json"), hooks)?;

        let hook = self.command(csm).arg("hook").output()?;
        ensure!(
            hook.status.success() && !hook.stdout.is_empty(),
            "fixture SessionStart hook produced no context"
        );
        fs::write(evidence.join("initial-hook.json"), hook.stdout)?;

        let mut config = String::from(
            "approval_policy = \"never\"\n\
             default_permissions = \"eval\"\n\
             cli_auth_credentials_store = \"file\"\n\
             [permissions.eval.filesystem]\n\
             \"/\" = \"read\"\n",
        );
        // The fixture can commit locally, while Codex still sandboxes writes
        // outside these disposable directories.
        for dir in [
            self.repo.clone(),
            self.repo.join(".git"),
            self.csm_home(),
            self.repo.parent().context("fixture root")?.join("tmp"),
        ] {
            config.push_str(&format!("{} = \"write\"\n", serde_json::to_string(&dir)?));
        }
        config.push_str("[shell_environment_policy]\nexclude = [\"OPENAI_API_KEY\"]\n");
        fs::write(self.codex_home.join("config.toml"), config)?;
        Ok(())
    }

    pub fn codex(&self, csm: &Path, api_key: Option<&OsStr>) -> Result<Command> {
        let mut command = self.command("codex");
        let path = std::env::join_paths(
            [csm.parent().context("csm binary directory")?.to_owned()]
                .into_iter()
                .chain(std::env::split_paths(
                    &std::env::var_os("PATH").unwrap_or_default(),
                )),
        )?;
        command.env("PATH", path);
        if let Some(key) = api_key {
            command.env("OPENAI_API_KEY", key);
        }
        Ok(command)
    }

    pub fn provision_auth(&self, api_key: Option<&OsStr>) -> Result<()> {
        if api_key.is_some() {
            return Ok(());
        }
        let auth = csm::inject::codex_dir()?.join("auth.json");
        ensure!(
            auth.is_file(),
            "Codex credentials missing; sign in with file-backed Codex auth or set OPENAI_API_KEY"
        );
        fs::copy(auth, self.codex_home.join("auth.json"))?;
        Ok(())
    }

    fn csm_home(&self) -> PathBuf {
        self.session
            .parent()
            .expect("sessions")
            .parent()
            .expect("csm home")
            .to_owned()
    }

    pub fn close(self) -> Result<()> {
        self.root.close().context("removing disposable workspace")
    }
}

pub fn relative_path(path: &str) -> Result<&Path> {
    let original = Path::new(path);
    ensure!(
        !path.is_empty()
            && original
                .components()
                .all(|part| matches!(part, Component::Normal(name) if name != ".git")),
        "invalid fixture path {path:?}"
    );
    Ok(original)
}

fn seed_files(root: &Path, files: &BTreeMap<String, String>) -> Result<()> {
    for (path, text) in files {
        let path = root.join(relative_path(path)?);
        fs::create_dir_all(path.parent().context("fixture file parent")?)?;
        fs::write(path, text)?;
    }
    Ok(())
}

pub fn run_process(
    mut command: Command,
    stdin: Stdio,
    stdout: File,
    stderr: File,
    timeout: Duration,
) -> Result<std::process::ExitStatus> {
    use std::os::unix::process::CommandExt;

    // A separate process group lets a timeout stop the turn and its shell commands.
    let mut child = command
        .process_group(0)
        .stdin(stdin)
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .with_context(|| format!("starting {}", command.get_program().to_string_lossy()))?;
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = Command::new("/bin/kill")
                .args(["-KILL", "--", &format!("-{}", child.id())])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            let _ = child.kill();
            child.wait()?;
            bail!("timed out after {} seconds", timeout.as_secs());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
