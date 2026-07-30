use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Linter that uppercases the entire contents of each file.
pub const UPPERCASE: &str =
    "sh -c 'for f in \"$@\"; do tr a-z A-Z < \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done' _";

/// Linter that replaces the first occurrence of `old` with `new` in each file.
pub fn replace(old: &str, new: &str) -> String {
    format!(
        "sh -c 'for f in \"$@\"; do sed \"s/{old}/{new}/\" \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done' _"
    )
}

/// Linter that creates `.git/linter-{n}` on start and blocks until it is deleted.
pub fn sentinel(n: u32) -> String {
    format!("sh -c 'touch .git/linter-{n} && while [ -f .git/linter-{n} ]; do sleep 0.1; done' ")
}

pub fn stagelint_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_stagelint"))
}

/// A temporary git repository for integration testing.
pub struct TestRepo {
    _temp_dir: TempDir,
    pub root: PathBuf,
    stagelint_exe: PathBuf,
}

impl TestRepo {
    /// Create a new temp git repo with no commits.
    pub fn empty() -> Self {
        let temp_dir = tempfile::Builder::new()
            .prefix("stagelint-test")
            .tempdir()
            .expect("create temp dir");
        let root = temp_dir.path().to_path_buf();
        let repo = TestRepo {
            _temp_dir: temp_dir,
            root,
            stagelint_exe: stagelint_exe(),
        };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.email", "test@test.com"]);
        repo.git(&["config", "user.name", "Test"]);
        repo.git(&["config", "core.autocrlf", "false"]);
        repo
    }

    /// Create a new temp git repo with an initial commit and config.
    pub fn new(config: &serde_json::Value) -> Self {
        let repo = Self::empty();
        let config_str = serde_json::to_string(&config).expect("serialize config");
        fs::write(repo.root.join(".stagelint.json"), config_str).expect("write config");
        repo.git(&["add", ".stagelint.json"]);
        repo.git(&["commit", "-m", "initial"]);
        repo
    }

    /// Write a file relative to repo root.
    pub fn write_file(&self, path: &str, content: &str) {
        let file_path = self.root.join(path);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent).expect("create parent dirs");
        }
        fs::write(&file_path, content).expect("write file");
    }

    /// Rename a file relative to repo root.
    pub fn rename_file(&self, from: &str, to: &str) {
        fs::rename(self.root.join(from), self.root.join(to)).expect("rename file");
    }

    /// Read a file relative to repo root.
    pub fn read_file(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).expect("read file")
    }

    /// Run a git command in this repo, asserting success.
    pub fn git(&self, args: &[&str]) -> String {
        let output = self.git_cmd(args);
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf8")
    }

    /// Run a git command in this repo without asserting success.
    /// Host git config is ignored so local settings cannot leak into tests.
    pub fn git_cmd(&self, args: &[&str]) -> Output {
        Command::new("git")
            .args(args)
            .current_dir(&self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join(".git/no-global-config"))
            .output()
            .unwrap_or_else(|e| panic!("git {}: {e}", args.join(" ")))
    }

    /// Poll for sentinel `n` (`.git/linter-{n}`) until it appears or `timeout` elapses.
    /// Returns `true` if the sentinel appeared, `false` if it did not.
    /// Pass `Duration::ZERO` for a non-blocking existence check.
    pub fn wait_sentinel(&self, n: u32, timeout: Duration) -> bool {
        let path = self.root.join(format!(".git/linter-{n}"));
        let deadline = Instant::now() + timeout;
        loop {
            if path.exists() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Unblock a sentinel linter by deleting `.git/linter-{n}`.
    pub fn release_sentinel(&self, n: u32) {
        fs::remove_file(self.root.join(format!(".git/linter-{n}"))).expect("release sentinel");
    }

    /// Spawn stagelint in this repo with captured output. Pass the returned child to
    /// `assert_success`, `assert_failure`, or use `.kill()` + `.wait()` for crash tests.
    /// Host git config is ignored so local settings cannot leak into tests.
    pub fn stagelint(&self, args: &[&str]) -> Child {
        Command::new(&self.stagelint_exe)
            .current_dir(&self.root)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.root.join(".git/no-global-config"))
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL")
            .env_remove("EMAIL")
            .spawn()
            .expect("spawn stagelint")
    }
}

/// Wait for stagelint to exit and assert it succeeded.
pub fn assert_success(child: Child) -> Output {
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "stagelint failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Wait for stagelint to exit and assert it failed.
pub fn assert_failure(child: Child) -> Output {
    let output = child.wait_with_output().expect("wait");
    assert!(
        !output.status.success(),
        "stagelint should have failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn symlink_file(target: &str, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_file(target, link);
}

pub fn symlink_dir(target: &str, link: &Path) -> io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    return std::os::windows::fs::symlink_dir(target, link);
}

pub fn file_symlinks_supported() -> bool {
    let dir = tempfile::tempdir().unwrap();
    let ok = fs::write(dir.path().join("target"), "").is_ok()
        && symlink_file("target", &dir.path().join("link")).is_ok();
    if !ok {
        skip("file symlinks unsupported");
    }
    ok
}

pub fn dir_symlinks_supported() -> bool {
    let dir = tempfile::tempdir().unwrap();
    let ok = fs::create_dir(dir.path().join("target")).is_ok()
        && symlink_dir("target", &dir.path().join("link")).is_ok();
    if !ok {
        skip("dir symlinks unsupported");
    }
    ok
}

/// CI must run every test: a skipped capability there is lost coverage, not a limited machine.
fn skip(reason: &str) {
    assert!(std::env::var_os("CI").is_none(), "{reason}");
    eprintln!("skipping test: {reason}");
}
