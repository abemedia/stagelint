mod cli;
mod config;
mod runner;
mod status;
mod workflow;

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};
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
    let (root, _trust) = gix::discover::upwards(Path::new("."))?;
    let (_, workdir) = root.into_repository_and_work_tree_directories();
    std::env::set_current_dir(workdir.ok_or_else(|| anyhow!("bare repository"))?)?;

    let repo = gix::open(".")?;
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

    let tasks = config::resolve(status.staged.iter().map(String::as_str))?;
    if tasks.is_empty() {
        return Ok(());
    }

    let cancel = CancellationToken::new();
    let mut runner = runner::Runner::new(opts.continue_on_error, opts.concurrent, cancel.clone());
    for task in &tasks {
        let commands: Vec<runner::Command> = task
            .commands
            .iter()
            .map(|obj| runner::Command {
                command: obj.command.clone(),
                pass_filenames: obj.pass_filenames,
            })
            .collect();
        let files: Vec<&str> = task.files.iter().map(String::as_str).collect();
        runner.add(&commands, &files)?;
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
    let repo = gix::discover(".")?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository: stagelint cannot install a pre-commit hook"))?
        .to_path_buf();
    let hooks_dir = {
        let config = repo.config_snapshot();
        if let Some(path) = config.path("core.hooksPath") {
            let dir = path
                .interpolate(gix::config::path::interpolate::Context {
                    home_dir: gix::path::env::home_dir().as_deref(),
                    ..Default::default()
                })
                .map_err(|e| anyhow!("failed to interpolate core.hooksPath: {e}"))?;
            workdir.join(dir)
        } else {
            repo.common_dir().join("hooks")
        }
    };
    fs::create_dir_all(&hooks_dir)?;
    let hook_path = hooks_dir.join("pre-commit");
    let hook_content = "#!/bin/sh\nstagelint\n";

    if !force && hook_path.symlink_metadata().is_ok() {
        let existing = fs::read(&hook_path).ok();
        if existing.as_deref() != Some(hook_content.as_bytes()) {
            bail!(
                "pre-commit hook already exists at {}\nUse --force to overwrite",
                hook_path.display()
            );
        }
    }

    match fs::remove_file(&hook_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    fs::write(&hook_path, hook_content)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))?;
    }

    eprintln!("Installed pre-commit hook at {}", hook_path.display());
    Ok(())
}
