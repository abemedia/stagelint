use std::collections::{BTreeMap, HashMap};
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;

use command_group::AsyncCommandGroup;
use tokio::io::AsyncReadExt;
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
    continue_on_error: bool,
    concurrent: usize,
    cancel: &CancellationToken,
) -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;
    let groups = plan(tasks, configs);
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
fn plan(tasks: &Reporter, configs: BTreeMap<PathBuf, Vec<Task>>) -> Vec<Group> {
    let mut groups = Vec::with_capacity(configs.len());
    for (path, config) in configs {
        let row = tasks.add(path.display().to_string());
        let mut jobs: Vec<Job> = Vec::with_capacity(config.len());
        // Last job to claim each file; the next claimant starts after it.
        let mut last_writer: HashMap<OsString, usize> = HashMap::new();
        for task in config {
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
            permits.acquire().await.ok()
        };
        if let Some(_permit) = ready.await {
            group.status(Status::Running);
            self.row.status(Status::Running);
            // Don't spawn a command a pending cancellation would instantly kill.
            while let Some((cmd, row)) = commands.next_if(|_| !running.is_cancelled()) {
                row.status(Status::Running);
                let (status, output) = run_command(&cmd, &self.files, &self.cwd, &running).await;
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
/// Returns how it ended with its output, stdout then stderr.
async fn run_command(
    cmd: &Cmd,
    files: &[OsString],
    cwd: &Path,
    running: &CancellationToken,
) -> (Status, Vec<u8>) {
    // `CreateProcess` only appends `.exe`, so `.cmd` and `.bat` shims need a full path.
    #[cfg(windows)]
    let program = &tokio::task::spawn_blocking({
        let name = cmd.program.clone();
        move || which::which(name).ok()
    })
    .await
    .ok()
    .flatten()
    .and_then(|p| p.into_os_string().into_string().ok())
    .unwrap_or_else(|| cmd.program.clone());
    #[cfg(not(windows))]
    let program = &cmd.program;

    let mut proc = tokio::process::Command::new(program);
    proc.args(&cmd.args)
        .args(if cmd.pass_filenames { files } else { &[] })
        .current_dir(cwd)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped());

    let mut child = match proc.group_spawn() {
        Ok(child) => child,
        Err(e) => {
            return (Status::Failed, format!("failed to run: {e}\n").into_bytes());
        }
    };

    let mut stdout = child.inner().stdout.take().unwrap();
    let mut stderr = child.inner().stderr.take().unwrap();

    // Owned buffers so the reader can resume after the kill to finish draining.
    let read = async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        out.extend_from_slice(&err);
        out
    };
    tokio::pin!(read);

    let mut cancelled = false;
    let mut handled = false;
    let mut output = loop {
        tokio::select! {
            biased;
            output = &mut read => break output,
            () = running.cancelled(), if !handled => {
                handled = true;
                // A process already exiting is no cancellation; keep its real status.
                cancelled = child.try_wait().ok().flatten().is_none();
                child.start_kill().ok();
            }
        }
    };

    let status = match child.wait().await {
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
        let result = run(&root, configs, false, 0, &CancellationToken::new());
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
        run(&root, configs, false, 1, &CancellationToken::new()).expect("run");
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
        let result = run(&root, configs, false, 0, &CancellationToken::new());
        assert!(matches!(result, Err(Error::Failed)));
        let log = lines(&buf);
        assert!(log.contains(&"[FAILED] *.a > false".to_owned()), "{log:?}");
        assert!(
            log.contains(&"[CANCELLED] *.a > true".to_owned()),
            "{log:?}"
        );
        assert_eq!(log.last().unwrap(), "[FAILED] cfg");
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
        run(&root, configs, false, 0, &CancellationToken::new()).expect("run");
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
