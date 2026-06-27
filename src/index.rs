use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::index::{entry, fs::Metadata, write};
use gix::{ObjectId, Repository};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read index")]
    IndexRead(#[source] gix::index::file::init::Error),
    #[error("failed to write index")]
    IndexWrite(#[source] gix::index::file::write::Error),
    #[error("failed to read {path}")]
    FileRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}")]
    FileWrite {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write blob for {path}")]
    BlobWrite {
        path: String,
        #[source]
        source: gix::object::write::Error,
    },
    #[error("failed to read object")]
    ObjectFind(#[source] gix::object::find::existing::Error),
}

/// A partially-staged file that the formatter changed.
pub struct MergeBase {
    /// Repo-relative path.
    pub path: String,
    /// Original staged content before formatting.
    pub base_oid: ObjectId,
    /// Index content after formatting.
    pub after_oid: ObjectId,
}

/// Returns `core.bigFileThreshold` in bytes (default 512 MiB).
pub fn big_file_threshold(repo: &Repository) -> u64 {
    repo.config_snapshot()
        .integer("core.bigFileThreshold")
        .and_then(|v| u64::try_from(v).ok())
        .unwrap_or(512 * 1024 * 1024)
}

/// Single-pass: detect formatter changes, update index OIDs in place, return merge bases.
///
/// For each staged file: checks index stat first (skip if reliably clean), then streams
/// to ODB. If the OID changed, updates the entry in place. If also partially staged,
/// records a `MergeBase`. Writes the index once at the end.
pub fn update(
    repo: &Repository,
    workdir: &Path,
    staged: &BTreeSet<String>,
    dirty: &BTreeSet<String>,
) -> Result<Vec<MergeBase>, Error> {
    let mut index = open_index(repo).map_err(Error::IndexRead)?;
    let mut merge_bases = Vec::new();
    let mut changed = false;

    for path in staged {
        let file_path = workdir.join(path);
        if !file_path.exists() {
            continue;
        }

        let bpath = path.as_bytes().as_bstr();
        let Some(pos) = index.entry_index_by_path_and_stage(bpath, entry::Stage::Unconflicted)
        else {
            continue;
        };

        if stat_is_clean(
            &file_path,
            &index.entries()[pos].stat,
            index.timestamp().seconds(),
        ) {
            continue;
        }

        let original_oid = index.entries()[pos].id;

        let file = fs::File::open(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;
        let new_oid = repo
            .write_blob_stream(file)
            .map_err(|e| Error::BlobWrite {
                path: path.to_owned(),
                source: e,
            })?
            .detach();

        if new_oid != original_oid {
            if dirty.contains(path) {
                merge_bases.push(MergeBase {
                    path: path.clone(),
                    base_oid: original_oid,
                    after_oid: new_oid,
                });
            }
            let entry = &mut index.entries_mut()[pos];
            entry.id = new_oid;
            if let Some(stat) = Metadata::from_path_no_follow(&file_path)
                .ok()
                .and_then(|m| entry::Stat::from_fs(&m).ok())
            {
                entry.stat = stat;
            }
            changed = true;
        }
    }

    if changed {
        index
            .write(write::Options::default())
            .map_err(Error::IndexWrite)?;
    }

    Ok(merge_bases)
}

/// Revert tracked files that a formatter modified as a side-effect (not staged, but changed on disk).
/// Skips any paths in `already_restored`.
pub fn revert_clean_tracked(
    repo: &Repository,
    workdir: &Path,
    already_restored: &[String],
) -> Result<(), Error> {
    let index = open_index(repo).map_err(Error::IndexRead)?;
    let skip: std::collections::HashSet<&str> =
        already_restored.iter().map(String::as_str).collect();

    for entry in index.entries() {
        let Ok(path) = std::str::from_utf8(entry.path(&index)) else {
            continue;
        };
        if skip.contains(path) {
            continue;
        }

        // Symlinks: indexed data is the target path, not file content. std::fs::read follows
        // the symlink and would compare/overwrite the target file - skip them entirely.
        if matches!(entry.mode, entry::Mode::SYMLINK | entry::Mode::COMMIT) {
            continue;
        }

        let file_path = workdir.join(path);
        if !file_path.exists() {
            continue;
        }

        if stat_is_clean(&file_path, &entry.stat, index.timestamp().seconds()) {
            continue;
        }

        let current = fs::read(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;
        let indexed = repo.find_object(entry.id).map_err(Error::ObjectFind)?;
        if current != indexed.data {
            fs::write(&file_path, &indexed.data).map_err(|e| Error::FileWrite {
                path: file_path,
                source: e,
            })?;
        }
    }

    Ok(())
}

/// Re-stat restored files and update their index entries so `git status`
/// doesn't flag them as dirty after we overwrote them.
pub fn refresh_stat(repo: &Repository, workdir: &Path, paths: &[String]) -> Result<(), Error> {
    if paths.is_empty() {
        return Ok(());
    }

    let mut index = open_index(repo).map_err(Error::IndexRead)?;
    let mut changed = false;

    for path in paths {
        let bpath = path.as_bytes().as_bstr();
        let Some(pos) = index.entry_index_by_path_and_stage(bpath, entry::Stage::Unconflicted)
        else {
            continue;
        };
        let file_path = workdir.join(path);
        let Ok(meta) = Metadata::from_path_no_follow(&file_path) else {
            continue;
        };
        let entry = &mut index.entries_mut()[pos];
        let Ok(stat) = entry::Stat::from_fs(&meta) else {
            continue;
        };
        entry.stat = stat;
        changed = true;
    }

    if changed {
        index
            .write(write::Options::default())
            .map_err(Error::IndexWrite)?;
    }

    Ok(())
}

/// Returns `true` when the on-disk stat matches `entry_stat` and can be
/// trusted. Requires mtime to predate `index_write_secs` to guard against
/// the racily-clean case on coarse-grained filesystems (HFS+ 1-second mtime).
fn stat_is_clean(path: &Path, entry_stat: &entry::Stat, index_write_secs: i64) -> bool {
    Metadata::from_path_no_follow(path)
        .ok()
        .and_then(|m| entry::Stat::from_fs(&m).ok())
        .is_some_and(|s| s == *entry_stat && i64::from(entry_stat.mtime.secs) < index_write_secs)
}

pub(crate) fn open_index(
    repo: &Repository,
) -> Result<gix::index::File, gix::index::file::init::Error> {
    gix::index::File::at(
        repo.index_path(),
        repo.object_hash(),
        false,
        gix::index::decode::Options::default(),
    )
}
