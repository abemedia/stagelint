mod cli;
mod config;
mod hook;
mod report;
mod runner;
mod status;
mod workflow;

use std::process::ExitCode;

use anyhow::{Context, Result, anyhow};
use tokio_util::sync::CancellationToken;

use cli::{Cli, Commands, Opts, StashScope};
use report::{Level, Reporter, Status};
use status::Source;

// musl's mallocng returns pages to the kernel eagerly; mimalloc keeps them, saving ~20%.
#[cfg(target_env = "musl")]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    let cli = Cli::parse();

    let level = match (cli.opts.quiet, cli.opts.verbose) {
        (true, _) => Level::Quiet,
        (_, true) => Level::Verbose,
        _ => Level::Normal,
    };
    let root = Reporter::new(level);
    let result = match cli.command {
        Some(Commands::Init { force, flags }) => hook::install(force, &flags, &root),
        None => run(&cli.opts, &root),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            // Skip errors that were already printed.
            let shown = matches!(
                err.downcast_ref::<runner::Error>(),
                Some(runner::Error::Failed | runner::Error::Cancelled)
            ) || matches!(
                err.downcast_ref::<workflow::Error>(),
                Some(workflow::Error::Restore)
            );
            if !shown {
                root.add(format!("Error: {err:#}")).status(Status::Failed);
            }
            ExitCode::FAILURE
        }
    }
}

fn run(opts: &Opts, root: &Reporter) -> Result<()> {
    let repo = gix::discover(std::env::current_dir()?)?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository"))?
        .to_path_buf();

    let stash_tracked = matches!(opts.stash, StashScope::Tracked | StashScope::Untracked);
    let stash_untracked = matches!(opts.stash, StashScope::Untracked);
    let source = if let Some(spec) = opts.diff.as_deref() {
        Source::Diff(spec)
    } else if opts.unstaged {
        Source::Unstaged
    } else if !opts.files.is_empty() {
        Source::Files(&opts.files)
    } else {
        Source::Staged
    };
    let noun = match source {
        Source::Staged => "staged",
        Source::Diff(_) => "changed",
        Source::Unstaged => "unstaged",
        Source::Files(_) => "given",
    };

    let status = status::collect(&repo, stash_untracked, source)?;
    if status.scope.is_empty() {
        root.add(format!("Could not find any {noun} files"))
            .status(Status::Warn);
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

    let configs = config::resolve(paths, &workdir)?;
    if configs.is_empty() {
        root.add(format!(
            "Could not find any {noun} files matching configured tasks"
        ))
        .status(Status::Warn);
        return Ok(());
    }

    // Cancelling rather than dying settles the tree, restores the cursor and protects the stash.
    let cancel = CancellationToken::new();
    ctrlc::set_handler({
        let c = cancel.clone();
        move || c.cancel()
    })?;

    let mut workflow = if source.stages_results() {
        Some(workflow::Workflow::new(
            &repo,
            &workdir,
            status,
            stash_tracked,
            root,
        )?)
    } else {
        None
    };

    let tasks = root
        .add(format!("Running tasks for {noun} files"))
        .status(Status::Running);
    let result = runner::run(
        &tasks,
        configs,
        &workdir,
        opts.continue_on_error,
        opts.concurrent,
        &cancel,
    );
    tasks.status(match &result {
        Ok(()) => Status::Done,
        Err(runner::Error::Cancelled) => Status::Cancelled,
        Err(_) => Status::Failed,
    });
    result?;

    if let Some(workflow) = &mut workflow {
        workflow.finish()?;
    }

    Ok(())
}
