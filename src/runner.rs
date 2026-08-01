use std::collections::HashMap;
use std::io::{self, Write};
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

struct CommandResult {
    status: Result<process::ExitStatus, io::Error>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    cancelled: bool,
}

struct TaskOutcome {
    task: usize,
    commands: Vec<CommandResult>,
}

/// A single command of a task pipeline.
pub struct Command {
    pub command: String,
    pub pass_filenames: bool,
}

pub struct Runner {
    tasks: Vec<Vec<(String, Vec<String>)>>,
    /// Tasks whose pending count drops when the task at the same index finishes.
    dependents: Vec<Vec<usize>>,
    /// Unfinished predecessors per task; a task is ready to start at zero.
    pending: Vec<usize>,
    /// Last task to claim each file; the next claimant starts after it.
    last_writer: HashMap<String, usize>,
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
            dependents: Vec::new(),
            pending: Vec::new(),
            last_writer: HashMap::new(),
            continue_on_error,
            concurrent,
            cancel,
            running,
        }
    }

    /// Parse command strings and build a task pipeline.
    /// `files` is appended to each command with `pass_filenames`. Tasks sharing files never run
    /// concurrently: each starts after the previous claimant of its files, in insertion order.
    pub fn add(&mut self, commands: &[Command], files: &[&str]) -> Result<(), Error> {
        if commands.is_empty() {
            return Ok(());
        }

        let mut resolved = Vec::with_capacity(commands.len());
        for command in commands {
            let mut args = shell_words::split(&command.command).map_err(|e| Error::Parse {
                command: command.command.clone(),
                source: e,
            })?;
            if args.is_empty() {
                return Err(Error::Empty);
            }
            if command.pass_filenames {
                args.extend(files.iter().map(ToString::to_string));
            }
            let program = args.remove(0);
            resolved.push((program, args));
        }

        let mut after: Vec<usize> = files
            .iter()
            .filter_map(|&file| self.last_writer.get(file).copied())
            .collect();
        after.sort_unstable();
        after.dedup();

        let id = self.tasks.len();
        self.tasks.push(resolved);
        self.dependents.push(Vec::new());
        self.pending.push(after.len());
        for &predecessor in &after {
            self.dependents[predecessor].push(id);
        }
        for &file in files {
            self.last_writer.insert(file.to_owned(), id);
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

    async fn run_async(self) -> Result<(), Error> {
        let mut set = JoinSet::new();
        let mut error_count = 0usize;
        let mut pending = self.pending.clone();
        let mut started = vec![false; self.tasks.len()];

        self.fill(&mut set, &mut started, &pending);

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

            for (cmd, result) in outcome.commands.iter().enumerate() {
                print_output(&result.stdout, &result.stderr);
                let name = &self.tasks[outcome.task][cmd].0;
                match &result.status {
                    Ok(s) if s.success() => {}
                    _ if result.cancelled => eprintln!("stagelint: command cancelled: {name}"),
                    Ok(s) => {
                        eprintln!("stagelint: command failed: {name} (exit {s})");
                        error_count += 1;
                    }
                    Err(e) => {
                        eprintln!("stagelint: failed to run {name}: {e}");
                        error_count += 1;
                    }
                }
            }

            if error_count > 0 && !self.continue_on_error {
                self.running.cancel();
            }
            for &dependent in &self.dependents[outcome.task] {
                pending[dependent] -= 1;
            }
            self.fill(&mut set, &mut started, &pending);
        }

        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if error_count > 0 {
            return Err(Error::CommandsFailed(error_count));
        }
        Ok(())
    }

    fn fill(&self, set: &mut JoinSet<TaskOutcome>, started: &mut [bool], pending: &[usize]) {
        for task in 0..self.tasks.len() {
            if self.concurrent != 0 && set.len() >= self.concurrent {
                return;
            }
            if !started[task] && pending[task] == 0 {
                self.spawn_task(set, task);
                started[task] = true;
            }
        }
    }

    fn spawn_task(&self, set: &mut JoinSet<TaskOutcome>, task: usize) {
        // Cancellation must not start a doomed task.
        if self.running.is_cancelled() {
            return;
        }

        let commands = self.tasks[task].clone();
        let running = self.running.clone();
        let continue_on_error = self.continue_on_error;

        set.spawn(async move {
            let mut results = Vec::with_capacity(commands.len());
            for (program, args) in &commands {
                // Don't spawn a command a pending cancellation would instantly kill.
                if running.is_cancelled() {
                    break;
                }
                let result = run_command(program, args, &running).await;
                let stop = result.cancelled
                    || (!continue_on_error && !matches!(&result.status, Ok(s) if s.success()));
                results.push(result);
                if stop {
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
async fn run_command(program: &str, args: &[String], running: &CancellationToken) -> CommandResult {
    let mut proc = tokio::process::Command::new(program);
    proc.args(args)
        .stdout(process::Stdio::piped())
        .stderr(process::Stdio::piped());

    let mut child = match proc.group_spawn() {
        Ok(child) => child,
        Err(e) => {
            return CommandResult {
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
                if child.try_wait().ok().flatten().is_none() {
                    child.start_kill().ok();
                    cancelled = true;
                }
            }
        }
    };

    let status = child.wait().await;
    CommandResult {
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
