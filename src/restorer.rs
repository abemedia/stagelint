use std::path::Path;

use anyhow::{Result, anyhow};
use gix::{ObjectId, Repository};

use crate::index;
use crate::stash;

/// Manages working tree restoration after formatters run.
///
/// Call `restore()` on the success path to restore stashed files, revert
/// formatter side-effects, and refresh index stats. If skipped (due to a
/// `?` early-return or linter failure), `Drop` calls it automatically.
///
/// Created regardless of whether a stash commit exists - stash restore is
/// a no-op when `oid` is `None`, but `revert_clean_tracked` still runs to
/// undo formatter side-effects on clean tracked files.
#[must_use]
pub struct Restorer<'a> {
    repo: &'a Repository,
    workdir: &'a Path,
    oid: Option<ObjectId>,
    stash_tracked: bool,
    absent_staged: Vec<String>, // materialized staged files to delete again once formatted
    attempted: bool, // restore() was called at least once; prevents Drop from retrying on failure
}

impl<'a> Restorer<'a> {
    pub fn new(
        repo: &'a Repository,
        workdir: &'a Path,
        oid: Option<ObjectId>,
        stash_tracked: bool,
        absent_staged: Vec<String>,
    ) -> Self {
        Self {
            repo,
            workdir,
            oid,
            stash_tracked,
            absent_staged,
            attempted: false,
        }
    }

    /// Restore stashed files, revert formatter side-effects on clean tracked
    /// files, and refresh index stats so restored files don't appear dirty.
    ///
    /// If stash restore fails, subsequent steps are skipped (they depend on
    /// knowing which files were restored). The stash ref is left intact for
    /// `git stash pop` recovery.
    pub fn restore(&mut self) -> Result<()> {
        self.attempted = true;

        let paths = if let Some(oid) = self.oid {
            stash::restore(self.repo, self.workdir, oid)?
        } else {
            Vec::new()
        };

        // Undo the materialization: these files must return to their deleted worktree state.
        for path in &self.absent_staged {
            let file_path = self.workdir.join(path);
            if let Err(e) = std::fs::remove_file(&file_path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(anyhow!("failed to delete {}: {e}", file_path.display()));
            }
        }

        if self.stash_tracked {
            index::revert_clean_tracked(self.repo, self.workdir, &paths)?;
        }

        if self.oid.is_some() {
            index::refresh_stat(self.repo, self.workdir, &paths)?;
        }

        Ok(())
    }
}

impl Drop for Restorer<'_> {
    fn drop(&mut self) {
        if self.attempted {
            return;
        }
        if let Err(e) = self.restore() {
            eprintln!("stagelint: warning: {e:#}");
        } else if let Some(oid) = self.oid
            && let Err(e) = stash::remove(self.repo, oid)
        {
            eprintln!("stagelint: warning: failed to drop stash ref: {e}");
        }
    }
}
