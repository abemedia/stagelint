use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;

use command_group::AsyncCommandGroup;
use tokio::io::AsyncReadExt;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to parse command: {command}")]
    Parse {
        command: String,
        #[source]
        source: shell_words::ParseError,
    },
    #[error("empty command")]
    Empty,
    #[error("failed to create async runtime")]
    Runtime(#[source] io::Error),
    #[error("{0} command(s) failed")]
    CommandsFailed(usize),
    #[error("cancelled")]
    Cancelled,
}

/// A single command of a task pipeline.
pub struct Command {
    pub command: String,
    pub pass_filenames: bool,
}

struct Cmd {
    command: String,
    program: String,
    args: Vec<String>,
    pass_filenames: bool,
}

struct CommandResult {
    name: String,
    status: Result<process::ExitStatus, io::Error>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    cancelled: bool,
}

struct TaskOutcome {
    task: usize,
    commands: Vec<CommandResult>,
}

struct Task {
    commands: Vec<Cmd>,
    /// Appended to the argv of each command with `pass_filenames`.
    files: Vec<OsString>,
    cwd: PathBuf,
    /// Tasks whose pending count drops when this one finishes.
    dependents: Vec<usize>,
    /// Unfinished predecessors; ready to start at zero.
    pending: usize,
    started: bool,
}

pub struct Runner {
    tasks: Vec<Task>,
    /// Last task to claim each file; the next claimant starts after it.
    last_writer: HashMap<OsString, usize>,
    continue_on_error: bool,
    concurrent: usize,
    cancel: CancellationToken,
    running: CancellationToken,
}

impl Runner {
    pub fn new(continue_on_error: bool, concurrent: usize, cancel: CancellationToken) -> Self {
        let running = cancel.child_token();
        Runner {
            tasks: Vec::new(),
            last_writer: HashMap::new(),
            continue_on_error,
            concurrent,
            cancel,
            running,
        }
    }

    /// Parse command strings and build a task pipeline.
    /// `files` is appended to each command with `pass_filenames` and the commands run in `cwd`.
    /// Tasks sharing files never run concurrently: each starts after the previous claimant of its
    /// files, in insertion order.
    pub fn add(
        &mut self,
        commands: impl IntoIterator<Item = Command>,
        files: Vec<OsString>,
        cwd: PathBuf,
    ) -> Result<(), Error> {
        let commands = commands.into_iter();
        let mut resolved = Vec::with_capacity(commands.size_hint().0);
        for Command {
            command,
            pass_filenames,
        } in commands
        {
            let mut args = shell_words::split(&command).map_err(|e| Error::Parse {
                command: command.clone(),
                source: e,
            })?;
            if args.is_empty() {
                return Err(Error::Empty);
            }
            let program = args.remove(0);
            if program.is_empty() {
                return Err(Error::Empty);
            }

            // CreateProcess appends only `.exe` to bare names; npm tools ship as `.cmd` shims.
            #[cfg(windows)]
            let program = which::which(&program)
                .ok()
                .and_then(|p| p.into_os_string().into_string().ok())
                .unwrap_or(program);

            resolved.push(Cmd {
                command,
                program,
                args,
                pass_filenames,
            });
        }
        if resolved.is_empty() {
            return Ok(());
        }

        let mut after: Vec<usize> = files
            .iter()
            .filter_map(|file| self.last_writer.get(file).copied())
            .collect();
        after.sort_unstable();
        after.dedup();

        let id = self.tasks.len();
        for file in &files {
            self.last_writer.insert(file.clone(), id);
        }
        self.tasks.push(Task {
            commands: resolved,
            files,
            cwd,
            dependents: Vec::new(),
            pending: after.len(),
            started: false,
        });
        for &predecessor in &after {
            self.tasks[predecessor].dependents.push(id);
        }

        Ok(())
    }

    pub fn run(self) -> Result<(), Error> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?
            .block_on(self.run_async())
    }

    async fn run_async(mut self) -> Result<(), Error> {
        let mut set = JoinSet::new();
        let mut error_count = 0usize;
        // Dependencies are resolved; holding a key per file would waste the run's lifetime.
        self.last_writer = HashMap::new();

        self.fill(&mut set);

        while let Some(r) = set.join_next().await {
            let outcome = match r {
                Ok(o) => o,
                Err(e) => {
                    eprintln!("stagelint: task panicked: {e}");
                    error_count += 1;
                    // A panic is a bug, not a command failure; continue_on_error does not apply.
                    self.running.cancel();
                    continue;
                }
            };

            for result in &outcome.commands {
                print_output(&result.stdout, &result.stderr);
                let name = &result.name;
                match &result.status {
                    Ok(s) if s.success() => {}
                    _ if result.cancelled => eprintln!("stagelint: command cancelled: `{name}`"),
                    Ok(s) => {
                        eprintln!("stagelint: command failed: `{name}` ({s})");
                        error_count += 1;
                    }
                    Err(e) => {
                        eprintln!("stagelint: failed to run `{name}`: {e}");
                        error_count += 1;
                    }
                }
            }

            if error_count > 0 && !self.continue_on_error {
                self.running.cancel();
            }
            let dependents = std::mem::take(&mut self.tasks[outcome.task].dependents);
            for dependent in dependents {
                self.tasks[dependent].pending -= 1;
            }
            self.fill(&mut set);
        }

        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if error_count > 0 {
            return Err(Error::CommandsFailed(error_count));
        }
        Ok(())
    }

    fn fill(&mut self, set: &mut JoinSet<TaskOutcome>) {
        for task in 0..self.tasks.len() {
            if self.concurrent != 0 && set.len() >= self.concurrent {
                return;
            }
            if !self.tasks[task].started && self.tasks[task].pending == 0 {
                self.spawn_task(set, task);
                self.tasks[task].started = true;
            }
        }
    }

    fn spawn_task(&mut self, set: &mut JoinSet<TaskOutcome>, task: usize) {
        // Cancellation must not start a doomed task.
        if self.running.is_cancelled() {
            return;
        }

        // Nothing reads these again, so hand them to the task rather than copying.
        let commands = std::mem::take(&mut self.tasks[task].commands);
        let files = std::mem::take(&mut self.tasks[task].files);
        let cwd = std::mem::take(&mut self.tasks[task].cwd);
        let running = self.running.clone();
        let continue_on_error = self.continue_on_error;

        set.spawn(async move {
            let mut results = Vec::with_capacity(commands.len());
            for cmd in commands {
                // Don't spawn a command a pending cancellation would instantly kill.
                if running.is_cancelled() {
                    break;
                }
                let result = run_command(cmd, &files, &cwd, &running).await;
                let failed = !matches!(&result.status, Ok(s) if s.success());
                results.push(result);
                if failed && !continue_on_error {
                    break;
                }
            }
            TaskOutcome {
                task,
                commands: results,
            }
        });
    }
}

/// Run one command to completion, killing and draining it if `running` is cancelled mid-flight.
async fn run_command(
    cmd: Cmd,
    files: &[OsString],
    cwd: &Path,
    running: &CancellationToken,
) -> CommandResult {
    let mut proc = tokio::process::Command::new(&cmd.program);
    proc.args(&cmd.args)
        .args(if cmd.pass_filenames { files } else { &[] })
        .current_dir(cwd)
        .stdin(process::Stdio::null())
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped());

    let mut child = match proc.group_spawn() {
        Ok(child) => child,
        Err(e) => {
            return CommandResult {
                name: cmd.command,
                status: Err(e),
                stdout: Vec::new(),
                stderr: Vec::new(),
                cancelled: false,
            };
        }
    };

    let mut stdout = child.inner().stdout.take().unwrap();
    let mut stderr = child.inner().stderr.take().unwrap();

    // Owned buffers so the reader can resume after the kill to finish draining.
    let read = async move {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let _ = tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        (out, err)
    };
    tokio::pin!(read);

    let mut cancelled = false;
    let mut handled = false;
    let (stdout, stderr) = loop {
        tokio::select! {
            biased;
            bufs = &mut read => break bufs,
            () = running.cancelled(), if !handled => {
                handled = true;
                // A process already exiting is no cancellation; keep its real status.
                cancelled = child.try_wait().ok().flatten().is_none();
                child.start_kill().ok();
            }
        }
    };

    let status = child.wait().await;
    CommandResult {
        name: cmd.command,
        status,
        stdout,
        stderr,
        cancelled,
    }
}

fn print_output(stdout: &[u8], stderr: &[u8]) {
    if !stdout.is_empty() {
        io::stdout().write_all(stdout).ok();
    }
    if !stderr.is_empty() {
        io::stderr().write_all(stderr).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runner_with(command: &str) -> Result<Runner, Error> {
        let mut runner = Runner::new(false, 0, CancellationToken::new());
        runner.add(
            [Command {
                command: command.to_owned(),
                pass_filenames: true,
            }],
            Vec::new(),
            PathBuf::from("."),
        )?;
        Ok(runner)
    }

    #[test]
    fn quoted_empty_program_rejected() {
        assert!(matches!(runner_with("''"), Err(Error::Empty)));
        assert!(matches!(runner_with("   "), Err(Error::Empty)));
    }

    #[test]
    fn unbalanced_quotes_rejected() {
        assert!(matches!(
            runner_with("echo 'unterminated"),
            Err(Error::Parse { .. })
        ));
    }

    #[test]
    fn command_not_found_fails() {
        let runner = runner_with("stagelint-no-such-program").expect("add");
        assert!(matches!(runner.run(), Err(Error::CommandsFailed(1))));
    }
}
