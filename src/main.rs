mod cli;
mod config;
mod runner;
mod status;
mod workflow;

use std::fs;

use anyhow::{Context, Result, anyhow, bail};
use clap::Parser;
use tokio_util::sync::CancellationToken;

use cli::{Cli, Commands, Opts, StashScope};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Init { force }) => init(force),
        None => run(&cli.opts),
    };

    if let Err(err) = result {
        eprintln!("stagelint: {err:#}");
        std::process::exit(1);
    }
}

fn run(opts: &Opts) -> Result<()> {
    let repo = gix::discover(std::env::current_dir()?)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository"))?
        .to_path_buf();

    let stash_tracked = matches!(opts.stash, StashScope::Tracked | StashScope::Untracked);
    let stash_untracked = matches!(opts.stash, StashScope::Untracked);

    let status = status::collect(&repo, stash_untracked)?;
    if status.staged.is_empty() {
        return Ok(());
    }

    // Git paths are bytes; everything downstream of here works in filesystem paths.
    let mut staged = Vec::with_capacity(status.staged.len());
    for path in &status.staged {
        staged.push(
            gix::path::try_from_byte_slice(path.as_slice())
                .with_context(|| format!("cannot represent {path} as a filesystem path"))?,
        );
    }

    let tasks = config::resolve(staged.iter().copied(), &workdir)?;
    if tasks.is_empty() {
        return Ok(());
    }

    let cancel = CancellationToken::new();
    let mut runner = runner::Runner::new(opts.continue_on_error, opts.concurrent, cancel.clone());
    for task in tasks {
        let commands = task.commands.into_iter().map(|obj| runner::Command {
            command: obj.command,
            pass_filenames: obj.pass_filenames,
        });
        runner.add(commands, task.files, task.cwd)?;
    }

    // Once we start stashing, Ctrl+C must not kill the process or it could result in data-loss.
    ctrlc::set_handler({
        let c = cancel.clone();
        move || c.cancel()
    })?;

    let mut workflow =
        workflow::Workflow::new(&repo, &workdir, status, stash_tracked, stash_untracked)?;

    runner.run()?;

    workflow.finish(opts.quiet)?;

    Ok(())
}

fn init(force: bool) -> Result<()> {
    let repo = gix::discover(std::env::current_dir()?)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository: stagelint cannot install a pre-commit hook"))?
        .to_path_buf();
    let hooks_dir = match repo
        .config_snapshot()
        .trusted_path("core.hooksPath")
        .map_err(|e| anyhow!("failed to interpolate core.hooksPath: {e}"))?
    {
        Some(dir) => workdir.join(dir),
        None => repo.common_dir().join("hooks"),
    };
    fs::create_dir_all(&hooks_dir)?;
    let hook_path = hooks_dir.join("pre-commit");
    let hook_content = "#!/bin/sh\nstagelint\n";

    if fs::read(&hook_path).ok().as_deref() != Some(hook_content.as_bytes()) {
        if !force && hook_path.symlink_metadata().is_ok() {
            bail!(
                "pre-commit hook already exists at {}\nUse --force to overwrite",
                hook_path.display()
            );
        }

        match fs::remove_file(&hook_path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        fs::write(&hook_path, hook_content)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))?;
    }

    eprintln!("Installed pre-commit hook at {}", hook_path.display());
    Ok(())
}
