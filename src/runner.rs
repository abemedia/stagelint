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
    #[error("{0} task(s) failed")]
    TasksFailed(usize),
    #[error("cancelled")]
    Cancelled,
}

struct Completion {
    task: usize,
    cmd: usize,
    status: Result<process::ExitStatus, io::Error>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

pub struct Runner {
    tasks: Vec<Vec<(String, Vec<String>)>>,
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
            continue_on_error,
            concurrent,
            cancel,
            running,
        }
    }

    /// Parse command strings and build a task pipeline.
    /// Each entry is a `(command, files)` pair; `files` is appended to that command's args.
    /// Pass an empty slice for commands that should not receive filenames.
    pub fn add(&mut self, pipeline: &[(&str, &[&str])]) -> Result<(), Error> {
        let mut resolved = Vec::with_capacity(pipeline.len());
        for (command, files) in pipeline {
            let mut args = shell_words::split(command).map_err(|e| Error::Parse {
                command: command.to_string(),
                source: e,
            })?;
            if args.is_empty() {
                return Err(Error::Empty);
            }
            args.extend(files.iter().map(ToString::to_string));
            let program = args.remove(0);
            resolved.push((program, args));
        }
        self.tasks.push(resolved);
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
        let mut next = 0;

        self.fill(&mut set, &mut next);

        // On cancellation, children kill themselves and no new ones spawn; the loop simply drains.
        while let Some(r) = set.join_next().await {
            let c = match r {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("stagelint: task panicked: {e}");
                    error_count += 1;
                    if !self.continue_on_error {
                        self.running.cancel();
                    }
                    continue;
                }
            };

            print_output(&c.stdout, &c.stderr);

            match c.status {
                Ok(s) if s.success() => {}
                Ok(s) if s.code().is_none() && self.running.is_cancelled() => {
                    eprintln!(
                        "stagelint: command cancelled: {}",
                        self.tasks[c.task][c.cmd].0
                    );
                }
                Ok(s) => {
                    eprintln!(
                        "stagelint: command failed: {} (exit {s})",
                        self.tasks[c.task][c.cmd].0
                    );
                    error_count += 1;
                }
                Err(e) => {
                    eprintln!(
                        "stagelint: failed to run {}: {e}",
                        self.tasks[c.task][c.cmd].0
                    );
                    error_count += 1;
                }
            }

            if error_count > 0 && !self.continue_on_error {
                self.running.cancel();
            }

            if c.cmd + 1 < self.tasks[c.task].len() {
                self.spawn_task(&mut set, c.task, c.cmd + 1);
            }
            self.fill(&mut set, &mut next);
        }

        if self.cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if error_count > 0 {
            return Err(Error::TasksFailed(error_count));
        }
        Ok(())
    }

    fn fill(&self, set: &mut JoinSet<Completion>, next: &mut usize) {
        while *next < self.tasks.len() && (self.concurrent == 0 || set.len() < self.concurrent) {
            if !self.tasks[*next].is_empty() {
                self.spawn_task(set, *next, 0);
            }
            *next += 1;
        }
    }

    fn spawn_task(&self, set: &mut JoinSet<Completion>, task: usize, cmd: usize) {
        // Cancellation must not spawn doomed commands.
        if self.running.is_cancelled() {
            return;
        }

        let (program, args) = &self.tasks[task][cmd];

        let mut proc = tokio::process::Command::new(program);
        proc.args(args)
            .stdout(process::Stdio::piped())
            .stderr(process::Stdio::piped());

        match proc.group_spawn() {
            Ok(mut child) => {
                let mut stdout = child.inner().stdout.take().unwrap();
                let mut stderr = child.inner().stderr.take().unwrap();
                let running = self.running.clone();

                set.spawn(async move {
                    let mut out = Vec::new();
                    let mut err = Vec::new();

                    tokio::select! {
                        biased;
                        () = running.cancelled() => { child.start_kill().ok(); }
                        _ = async {
                            tokio::join!(
                                stdout.read_to_end(&mut out),
                                stderr.read_to_end(&mut err),
                            )
                        } => {}
                    }

                    let status = child.wait().await;
                    Completion {
                        task,
                        cmd,
                        status,
                        stdout: out,
                        stderr: err,
                    }
                });
            }
            Err(e) => {
                // Route spawn failures through the completion path like exit failures.
                set.spawn(async move {
                    Completion {
                        task,
                        cmd,
                        status: Err(e),
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    }
                });
            }
        }
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
