mod cli;
mod config;
mod index;
mod lockfile;
mod merge;
mod restorer;
mod runner;
mod stash;
mod status;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

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
    let repo = gix::discover(".")?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| anyhow!("bare repository"))?
        .to_path_buf();
    std::env::set_current_dir(&workdir)?;

    let stash_tracked = matches!(opts.stash, StashScope::Tracked | StashScope::All);
    let stash_untracked = matches!(opts.stash, StashScope::Untracked | StashScope::All);

    let status = status::collect(&repo, stash_untracked)?;
    if status.staged.is_empty() {
        return Ok(());
    }

    let cancel = CancellationToken::new();
    let runner = build_runner(&workdir, &status, opts, cancel.clone())?;

    // Once we start stashing, Ctrl+C must not kill the process or it could result in data-loss.
    ctrlc::set_handler({
        let c = cancel.clone();
        move || c.cancel()
    })?;

    let stash_oid = stash::save(&repo, &workdir, &status, stash_tracked, stash_untracked)?;
    let absent_staged: Vec<String> = status
        .staged
        .intersection(&status.missing)
        .cloned()
        .collect();
    let mut restorer =
        restorer::Restorer::new(&repo, &workdir, stash_oid, stash_tracked, absent_staged);

    if let Err(e) = runner.run() {
        bail!(e);
    }

    let merge_bases = index::update(&repo, &workdir, &status.staged, &status.dirty)?;

    restorer.restore()?;
    apply_merges(&repo, &workdir, opts.quiet, &merge_bases)?;

    if let Some(oid) = stash_oid {
        stash::remove(&repo, oid)?;
    }

    Ok(())
}

fn build_runner(
    workdir: &Path,
    status: &status::WorktreeStatus,
    opts: &Opts,
    cancel: CancellationToken,
) -> Result<runner::Runner> {
    let mut files_by_config: BTreeMap<PathBuf, Vec<&str>> = BTreeMap::new();
    let mut cache = HashMap::new();

    for path in &status.staged {
        let file_dir = workdir.join(path).parent().unwrap_or(workdir).to_path_buf();
        if let Some(config_path) = config::find(&file_dir, workdir, &mut cache) {
            files_by_config.entry(config_path).or_default().push(path);
        }
    }

    if files_by_config.is_empty() {
        bail!("{}", config::Error::NotFound);
    }

    let mut r = runner::Runner::new(opts.continue_on_error, opts.concurrent, cancel);

    for (config_path, paths) in &files_by_config {
        let cfg = config::load_file(config_path)?;
        let config_dir = config_path.parent().unwrap_or(workdir);
        let prefix = config_dir.strip_prefix(workdir).unwrap_or(Path::new(""));

        for (pattern, entry) in &cfg {
            let matcher = globset::Glob::new(pattern)
                .map_err(|e| anyhow!("invalid glob pattern '{pattern}': {e}"))?
                .compile_matcher();

            let matching: Vec<&str> = paths
                .iter()
                .copied()
                .filter(|p| {
                    let relative = Path::new(p).strip_prefix(prefix).unwrap_or(Path::new(p));
                    matcher.is_match(relative)
                })
                .collect();

            if matching.is_empty() {
                continue;
            }

            let pipeline: Vec<(&str, &[&str])> = entry
                .iter()
                .map(|obj| {
                    let files: &[&str] = if obj.pass_filenames { &matching } else { &[] };
                    (obj.command.as_str(), files)
                })
                .collect();
            r.add(&pipeline)?;
        }
    }

    Ok(r)
}

fn apply_merges(
    repo: &gix::Repository,
    workdir: &Path,
    quiet: bool,
    merge_bases: &[index::MergeBase],
) -> Result<()> {
    let threshold = index::big_file_threshold(repo);

    for mb in merge_bases {
        let file_path = workdir.join(&mb.path);
        if !file_path.exists() {
            continue;
        }

        let file_size = file_path.metadata().map_or(0, |m| m.len());
        if file_size > threshold {
            if !quiet {
                eprintln!(
                    "stagelint: warning: could not apply formatting to working tree for \
                     {}: file exceeds core.bigFileThreshold - staged content is \
                     formatted, working tree unchanged",
                    mb.path
                );
            }
            continue;
        }

        let base = repo
            .find_object(mb.base_oid)
            .map_err(|e| anyhow!("failed to read base blob for {}: {e}", mb.path))?;
        let after = repo
            .find_object(mb.after_oid)
            .map_err(|e| anyhow!("failed to read after blob for {}: {e}", mb.path))?;
        let working = fs::read(&file_path)?;

        match merge::three_way_merge(workdir, &mb.path, &working, &base.data, &after.data) {
            Ok(()) => {}
            Err(merge::Error::AutoResolved) => {
                if !quiet {
                    eprintln!(
                        "stagelint: warning: could not apply all formatting changes to {}",
                        mb.path
                    );
                }
            }
            Err(e) => bail!("failed to merge {}: {e}", mb.path),
        }
    }

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
                .map_err(|e| anyhow!("failed to interpolate core.hooksPath: {e}"))?
                .into_owned();
            workdir.join(dir)
        } else {
            repo.common_dir().join("hooks")
        }
    };
    fs::create_dir_all(&hooks_dir)?;
    let hook_path = hooks_dir.join("pre-commit");
    let hook_content = "#!/bin/sh\nstagelint\n";

    if !force && hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)?;
        if existing != hook_content {
            bail!(
                "pre-commit hook already exists at {}\nUse --force to overwrite",
                hook_path.display()
            );
        }
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
