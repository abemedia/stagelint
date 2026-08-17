mod cli;
mod config;
mod hook;
mod runner;
mod status;
mod workflow;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use tokio_util::sync::CancellationToken;

use cli::{Cli, Commands, Opts, StashScope};

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Commands::Init { force }) => hook::install(force),
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
    if status.scope.is_empty() {
        if !opts.quiet {
            eprintln!("stagelint: warning: could not find any staged files");
        }
        return Ok(());
    }

    // Git paths are bytes; everything downstream of here works in filesystem paths.
    let mut paths = Vec::with_capacity(status.scope.len());
    for path in &status.scope {
        paths.push(
            gix::path::try_from_byte_slice(path.as_slice())
                .with_context(|| format!("cannot represent {path} as a filesystem path"))?,
        );
    }

    let tasks = config::resolve(paths, &workdir)?;
    if tasks.is_empty() {
        if !opts.quiet {
            eprintln!(
                "stagelint: warning: could not find any staged files matching configured tasks"
            );
        }
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

    let mut workflow = workflow::Workflow::new(&repo, &workdir, status, stash_tracked, opts.quiet)?;

    runner.run()?;

    workflow.finish()?;

    Ok(())
}
