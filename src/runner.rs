use std::collections::{BTreeMap, HashMap};
use std::env;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use command_group::AsyncCommandGroup;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::config::{Cmd, Task};
use crate::report::{Reporter, Status};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to create async runtime")]
    Runtime(#[source] io::Error),
    #[error("failed")]
    Failed,
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    Panicked(tokio::task::JoinError),
}

/// Run every task of every config, drawing the tree under `tasks`. Tasks sharing files never
/// run concurrently; they start in declaration order.
pub fn run(
    tasks: &Reporter,
    configs: BTreeMap<PathBuf, Vec<Task>>,
    workdir: &Path,
    continue_on_error: bool,
    concurrent: usize,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;
    let groups = plan(tasks, configs, workdir);
    let running = cancel.child_token();
    let permits = Arc::new(Semaphore::new(if concurrent == 0 {
        Semaphore::MAX_PERMITS
    } else {
        concurrent
    }));
    runtime.block_on(async {
        let mut set = JoinSet::new();
        for group in groups {
            let (permits, running) = (permits.clone(), running.clone());
            set.spawn(group.run(permits, running, continue_on_error));
        }

        let mut worst = Status::Done;
        let mut panic = None;
        while let Some(joined) = set.join_next().await {
            match joined.and_then(|status| status) {
                Ok(status) => worst = worst.max(status),
                Err(e) => {
                    // A panic is a bug, not a command failure; continue_on_error does not apply.
                    running.cancel();
                    panic.get_or_insert(e);
                }
            }
        }

        if let Some(e) = panic {
            return Err(Error::Panicked(e));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if worst == Status::Failed {
            return Err(Error::Failed);
        }
        Ok(())
    })
}

/// Create the rows under `tasks`, in tree order, and the groups of jobs that drive them.
fn plan(tasks: &Reporter, configs: BTreeMap<PathBuf, Vec<Task>>, workdir: &Path) -> Vec<Group> {
    let inherited = env::var_os("PATH");
    let mut paths: HashMap<PathBuf, Option<OsString>> = HashMap::new();
    let mut groups = Vec::with_capacity(configs.len());
    for (path, config) in configs {
        let row = tasks.add(path.display().to_string());
        let mut jobs: Vec<Job> = Vec::with_capacity(config.len());
        // Last job to claim each file; the next claimant starts after it.
        let mut last_writer: HashMap<OsString, usize> = HashMap::new();
        for task in config {
            let local = paths
                .entry(task.cwd.clone())
                .or_insert_with(|| local_path(&task.cwd, workdir, inherited.as_deref()))
                .clone();
            let n = task.files.len();
            let glob = row
                .add(task.pattern)
                .note(format!("{n} file{}", if n == 1 { "" } else { "s" }));
            let commands = task
                .commands
                .into_iter()
                .map(|cmd| {
                    let row = glob.add(&cmd.line);
                    (cmd, row)
                })
                .collect();
            let mut predecessors: Vec<usize> = task
                .files
                .iter()
                .filter_map(|file| last_writer.get(file).copied())
                .collect();
            predecessors.sort_unstable();
            predecessors.dedup();
            for file in &task.files {
                last_writer.insert(file.clone(), jobs.len());
            }
            let (done, _) = watch::channel(());
            jobs.push(Job {
                row: glob,
                commands,
                files: task.files,
                cwd: task.cwd,
                path: local,
                after: predecessors
                    .iter()
                    .map(|&p| jobs[p].done.subscribe())
                    .collect(),
                done,
            });
        }
        groups.push(Group { row, jobs });
    }
    groups
}

/// Project-local tool directories from `cwd` up to `workdir`, nearest first, then inherited PATH.
fn local_path(cwd: &Path, workdir: &Path, inherited: Option<&OsStr>) -> Option<OsString> {
    #[cfg(windows)]
    const LOCAL_BIN: &[&str] = &["node_modules/.bin", "vendor/bin", ".venv/Scripts"];
    #[cfg(not(windows))]
    const LOCAL_BIN: &[&str] = &["node_modules/.bin", "vendor/bin", ".venv/bin"];

    let dirs: Vec<PathBuf> = cwd
        .ancestors()
        .take_while(|dir| dir.starts_with(workdir))
        .flat_map(|dir| LOCAL_BIN.iter().map(move |name| dir.join(name)))
        .filter(|dir| dir.is_dir())
        .collect();
    if dirs.is_empty() {
        return None;
    }
    let inherited = inherited.into_iter().flat_map(env::split_paths);
    env::join_paths(dirs.into_iter().chain(inherited)).ok()
}

/// One config's row and the jobs under it.
struct Group {
    row: Reporter,
    jobs: Vec<Job>,
}

impl Group {
    /// Run the jobs and settle the row to the worst of them.
    async fn run(
        self,
        permits: Arc<Semaphore>,
        running: CancellationToken,
        continue_on_error: bool,
    ) -> Result<Status, tokio::task::JoinError> {
        let mut set = JoinSet::new();
        for job in self.jobs {
            let (row, permits, running) = (self.row.clone(), permits.clone(), running.clone());
            set.spawn(job.run(row, permits, running, continue_on_error));
        }
        let mut worst = Status::Done;
        let mut panic = None;
        while let Some(joined) = set.join_next().await {
            match joined {
                Ok(status) => worst = worst.max(status),
                Err(e) => {
                    running.cancel();
                    panic.get_or_insert(e);
                }
            }
        }
        if let Some(e) = panic {
            return Err(e);
        }
        self.row.status(worst);
        Ok(worst)
    }
}

/// One pattern's commands, each with its row.
struct Job {
    row: Reporter,
    commands: Vec<(Cmd, Reporter)>,
    files: Vec<OsString>,
    cwd: PathBuf,
    path: Option<OsString>,
    /// Completion signals from the jobs this one must follow; a closed channel skips this job.
    after: Vec<watch::Receiver<()>>,
    /// Signals this job finished. A panic drops it unsent, closing the channel.
    done: watch::Sender<()>,
}

impl Job {
    /// Run the commands in order after the predecessors; returns the worst status.
    async fn run(
        mut self,
        group: Reporter,
        permits: Arc<Semaphore>,
        running: CancellationToken,
        continue_on_error: bool,
    ) -> Status {
        let mut worst = Status::Done;
        let mut commands = self.commands.into_iter().peekable();
        let ready = async {
            for rx in &mut self.after {
                rx.changed().await.ok()?;
            }
            tokio::select! {
                // Cancelled wins a tie, so a cancelled task never starts on a free permit.
                biased;
                () = running.cancelled() => None,
                permit = permits.acquire() => permit.ok(),
            }
        };
        let path = self.path.as_deref();
        if let Some(_permit) = ready.await {
            group.status(Status::Running);
            self.row.status(Status::Running);
            // Don't spawn a command a pending cancellation would instantly kill.
            while let Some((cmd, row)) = commands.next_if(|_| !running.is_cancelled()) {
                row.status(Status::Running);
                let (status, output) =
                    run_command(&cmd, &self.files, &self.cwd, path, &running).await;
                row.output(output).status(status);
                worst = worst.max(status);
                if worst == Status::Failed && !continue_on_error {
                    running.cancel();
                }
            }
        }
        for (_, row) in commands {
            row.status(Status::Cancelled);
            worst = Status::Cancelled.max(worst);
        }
        self.row.status(worst);
        self.done.send_replace(());
        worst
    }
}

/// Run one command to completion, killing and draining it if `running` is cancelled mid-flight.
/// Returns how it ended with its output, both streams interleaved as the child wrote them.
async fn run_command(
    cmd: &Cmd,
    files: &[OsString],
    cwd: &Path,
    path: Option<&OsStr>,
    running: &CancellationToken,
) -> (Status, Vec<u8>) {
    // `CreateProcess` only appends `.exe`, so `.cmd` and `.bat` shims need a full path.
    #[cfg(windows)]
    let program = &tokio::task::spawn_blocking({
        let name = cmd.program.clone();
        let path = path.map_or_else(|| env::var_os("PATH"), |path| Some(path.to_os_string()));
        let cwd = cwd.to_path_buf();
        move || which::which_in(name, path, cwd).ok()
    })
    .await
    .ok()
    .flatten()
    .map_or_else(|| cmd.program.clone().into(), PathBuf::into_os_string);
    #[cfg(not(windows))]
    let program = &cmd.program;

    // One pipe for both streams, so writes arrive in the order the child made them.
    let (mut reader, out, err) = match io::pipe().and_then(|(r, w)| Ok((r, w.try_clone()?, w))) {
        Ok(pipe) => pipe,
        Err(e) => return (Status::Failed, format!("failed to run: {e}\n").into_bytes()),
    };

    let mut child = {
        let mut proc = tokio::process::Command::new(program);
        proc.args(&cmd.args)
            .args(if cmd.pass_filenames { files } else { &[] })
            .current_dir(cwd)
            .stdin(process::Stdio::null())
            .stdout(out)
            .stderr(err);
        if let Some(path) = path {
            proc.env("PATH", path);
        }
        match proc.group_spawn() {
            Ok(child) => child,
            Err(e) => {
                return (Status::Failed, format!("failed to run: {e}\n").into_bytes());
            }
        }
        // Dropping `proc` closes our write ends; without that the read never sees EOF.
    };

    // `PipeReader` is not async. The read drains what is left after the kill, then ends at EOF.
    let read = tokio::task::spawn_blocking(move || {
        let mut buf = Vec::new();
        io::Read::read_to_end(&mut reader, &mut buf).ok();
        buf
    });
    tokio::pin!(read);

    let mut cancelled = false;
    let mut handled = false;
    let mut drained = false;
    let mut output = Vec::new();
    // EOF is not the child exiting: a process can close its descriptors and keep running.
    let exit = loop {
        let interrupted = tokio::select! {
            biased;
            bytes = &mut read, if !drained => {
                output = bytes.unwrap_or_default();
                drained = true;
                false
            }
            exit = child.wait(), if drained => break exit,
            () = running.cancelled(), if !handled => true,
        };
        if interrupted {
            handled = true;
            // A process already exiting is no cancellation; keep its real status.
            cancelled = child.try_wait().ok().flatten().is_none();
            child.start_kill().ok();
        }
    };

    let status = match exit {
        Ok(status) if status.success() => Status::Done,
        Ok(status) if cancelled && (cfg!(windows) || status.code().is_none()) => Status::Cancelled,
        Err(_) if cancelled => Status::Cancelled,
        Ok(status) => {
            output.extend_from_slice(format!("{status}\n").as_bytes());
            Status::Failed
        }
        Err(e) => {
            output.extend_from_slice(format!("failed to run: {e}\n").as_bytes());
            Status::Failed
        }
    };
    (status, output)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use super::*;
    use crate::report::Level;

    /// A plain-mode reporter whose log can be read back.
    fn reporter() -> (Reporter, Arc<Mutex<Vec<u8>>>) {
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl io::Write for Buf {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        console::set_colors_enabled_stderr(false);
        let buf = Arc::new(Mutex::new(Vec::new()));
        let root = Reporter::custom(
            Level::Normal,
            indicatif::ProgressDrawTarget::hidden(),
            Box::new(Buf(buf.clone())),
            None,
            None,
        );
        (root, buf)
    }

    fn task(pattern: &str, lines: &[&str], files: &[&str]) -> Task {
        Task {
            pattern: pattern.to_owned(),
            commands: lines
                .iter()
                .map(|line| {
                    let mut args = shell_words::split(line).unwrap();
                    Cmd {
                        line: (*line).to_owned(),
                        program: args.remove(0),
                        args,
                        pass_filenames: false,
                    }
                })
                .collect(),
            files: files.iter().map(OsString::from).collect(),
            cwd: PathBuf::from("."),
        }
    }

    fn lines(buf: &Mutex<Vec<u8>>) -> Vec<String> {
        String::from_utf8(buf.lock().unwrap().clone())
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn command_not_found_fails() {
        let (root, buf) = reporter();
        let configs = BTreeMap::from([(
            PathBuf::from("cfg"),
            vec![task("*", &["stagelint-no-such-program"], &[])],
        )]);
        let result = run(
            &root,
            configs,
            Path::new("."),
            false,
            0,
            &CancellationToken::new(),
        );
        assert!(matches!(result, Err(Error::Failed)));
        let log = lines(&buf);
        let failed = log
            .iter()
            .position(|l| l == "[FAILED] * > stagelint-no-such-program")
            .expect("failed line");
        assert!(
            log.get(failed + 1)
                .is_some_and(|l| l.starts_with("failed to run: ")),
            "{log:?}"
        );
        assert_eq!(log.last().unwrap(), "[FAILED] cfg");
    }

    /// A config row starts with its first task and settles after its last, as its tasks did.
    #[test]
    fn config_row_follows_its_tasks() {
        let (root, buf) = reporter();
        let configs = BTreeMap::from([(
            PathBuf::from("cfg"),
            vec![task("*.a", &["true"], &[]), task("*.b", &["true"], &[])],
        )]);
        run(
            &root,
            configs,
            Path::new("."),
            false,
            1,
            &CancellationToken::new(),
        )
        .expect("run");
        let log = lines(&buf);
        assert_eq!(log.first().unwrap(), "[STARTED] cfg");
        assert_eq!(log.last().unwrap(), "[COMPLETED] cfg");
        assert_eq!(log.iter().filter(|l| l.ends_with("] cfg")).count(), 2);
    }

    /// A failure stops the run: later commands are cancelled, the config and the run fail.
    #[test]
    fn failure_cancels_the_rest() {
        let (root, buf) = reporter();
        let configs = BTreeMap::from([(
            PathBuf::from("cfg"),
            vec![task("*.a", &["false", "true"], &[])],
        )]);
        let result = run(
            &root,
            configs,
            Path::new("."),
            false,
            0,
            &CancellationToken::new(),
        );
        assert!(matches!(result, Err(Error::Failed)));
        let log = lines(&buf);
        assert!(log.contains(&"[FAILED] *.a > false".to_owned()), "{log:?}");
        assert!(
            log.contains(&"[CANCELLED] *.a > true".to_owned()),
            "{log:?}"
        );
        assert_eq!(log.last().unwrap(), "[FAILED] cfg");
    }

    #[test]
    fn local_path_walks_ancestors_within_the_worktree() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let root = workspace.path().join("repo");
        let cwd = root.join("packages/app");
        fs::create_dir_all(cwd.join("vendor/bin")).expect("create vendor/bin");
        fs::create_dir_all(root.join("node_modules/.bin")).expect("create .bin");
        // Above the worktree root.
        fs::create_dir_all(workspace.path().join("node_modules/.bin")).expect("create outer .bin");

        let inherited = env::join_paths(["/usr/bin", "/bin"]).expect("join");
        let path = local_path(&cwd, &root, Some(&inherited)).expect("path");
        let dirs: Vec<PathBuf> = env::split_paths(&path).collect();
        assert_eq!(
            dirs,
            [
                cwd.join("vendor/bin"),
                root.join("node_modules/.bin"),
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
            ],
            "{dirs:?}"
        );
    }

    /// `None` leaves the child's PATH unset rather than empty, which `execvp` treats differently.
    #[test]
    fn local_path_without_local_dirs_is_none() {
        let workspace = tempfile::tempdir().expect("tempdir");
        let root = workspace.path();
        let inherited = OsStr::new("/usr/bin:/bin");
        assert_eq!(local_path(root, root, Some(inherited)), None);
        assert_eq!(local_path(root, root, None), None);
    }

    #[cfg(unix)]
    #[test]
    fn commands_resolve_against_local_bin() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().expect("tempdir");
        let root = workspace.path().join("repo");
        let cwd = root.join("packages/app");
        fs::create_dir_all(&cwd).expect("create package");
        let bin = root.join("node_modules/.bin");
        fs::create_dir_all(&bin).expect("create .bin");
        let tool = bin.join("stagelint-fake-linter");
        fs::write(&tool, "#!/bin/sh\nexit 0\n").expect("write tool");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut task = task("*", &["stagelint-fake-linter"], &[]);
        task.cwd = cwd;
        let (tasks, buf) = reporter();
        let configs = BTreeMap::from([(PathBuf::from("cfg"), vec![task])]);
        run(&tasks, configs, &root, false, 0, &CancellationToken::new()).expect("run");
        assert_eq!(lines(&buf).last().unwrap(), "[COMPLETED] cfg");
    }

    #[cfg(unix)]
    #[test]
    fn commands_ignore_local_bin_above_the_worktree() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = tempfile::tempdir().expect("tempdir");
        let root = workspace.path().join("repo");
        let cwd = root.join("packages/app");
        fs::create_dir_all(&cwd).expect("create package");
        // Above the worktree root.
        let bin = workspace.path().join("node_modules/.bin");
        fs::create_dir_all(&bin).expect("create .bin");
        let tool = bin.join("stagelint-fake-linter");
        fs::write(&tool, "#!/bin/sh\nexit 0\n").expect("write tool");
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod");

        let mut task = task("*", &["stagelint-fake-linter"], &[]);
        task.cwd = cwd;
        let (tasks, buf) = reporter();
        let configs = BTreeMap::from([(PathBuf::from("cfg"), vec![task])]);
        let result = run(&tasks, configs, &root, false, 0, &CancellationToken::new());
        assert!(matches!(result, Err(Error::Failed)));
        assert_eq!(lines(&buf).last().unwrap(), "[FAILED] cfg");
    }

    /// Tasks that share a file run one after the other, in declaration order.
    #[test]
    fn shared_files_serialise_tasks() {
        let (root, buf) = reporter();
        let configs = BTreeMap::from([(
            PathBuf::from("cfg"),
            vec![
                task("*.a", &["sleep 0.2"], &["x"]),
                task("*.b", &["true"], &["x"]),
            ],
        )]);
        run(
            &root,
            configs,
            Path::new("."),
            false,
            0,
            &CancellationToken::new(),
        )
        .expect("run");
        let log = lines(&buf);
        let done_a = log
            .iter()
            .position(|l| l == "[COMPLETED] *.a > sleep 0.2")
            .unwrap_or_else(|| panic!("*.a never completed: {log:?}"));
        let start_b = log
            .iter()
            .position(|l| l == "[STARTED] *.b > true")
            .unwrap_or_else(|| panic!("*.b never started: {log:?}"));
        assert!(done_a < start_b, "{log:?}");
    }
}
