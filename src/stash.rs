use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use gix::bstr::ByteSlice;
use gix::index::entry::Stage;
use gix::objs::Commit;
use gix::objs::tree::{EntryKind, EntryMode};
use gix::refs::Target;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::{Id, ObjectId, Repository};

use crate::index::open_index;
use crate::lockfile::LockFile;
use crate::status::WorktreeStatus;

/// A file captured for stashing: its repo-relative path, blob OID, and tree entry mode.
struct StashEntry {
    path: String,
    oid: ObjectId,
    mode: EntryMode,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read tree from commit")]
    TreeDecode(#[source] gix::object::commit::Error),
    #[error("failed to create tree editor")]
    TreeEditInit(#[source] gix::object::tree::editor::init::Error),
    #[error("failed to edit tree")]
    TreeEdit(#[source] gix::objs::tree::editor::Error),
    #[error("failed to write edited tree")]
    TreeEditorWrite(#[source] gix::object::tree::editor::write::Error),
    #[error("failed to write commit")]
    CommitWrite(#[source] gix::object::write::Error),
    #[error("failed to create stash ref")]
    RefWrite(#[source] gix::reference::edit::Error),
    #[error("failed to delete stash ref")]
    RefDelete(#[source] gix::reference::edit::Error),
    #[error("failed to read index")]
    IndexRead(#[source] gix::index::file::init::Error),
    #[error("failed to read object")]
    ObjectFind(#[source] gix::object::find::existing::Error),
    #[error("failed to diff trees")]
    TreeDiff(#[source] Box<dyn std::error::Error + Send + Sync>),
    #[error("non-utf8 path in tree")]
    NonUtf8Path,
    #[error("could not determine committer identity; set user.name and user.email in git config")]
    NoIdentity,
    #[error("invalid committer time")]
    CommitterTime(#[source] gix::config::time::Error),
    #[error("invalid committer signature")]
    CommitterValidation(#[source] gix::date::Error),
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
    #[error(transparent)]
    Lock(#[from] crate::lockfile::Error),
    #[error("failed to delete {path}")]
    FileDelete {
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
    #[error("failed to read blob for {path}")]
    BlobRead {
        path: String,
        #[source]
        source: gix::object::find::existing::Error,
    },
}

/// Collect files to stash, create the stash commit, and hide stashed files.
/// Returns the stash commit OID, or `None` if nothing needed stashing.
pub fn save(
    repo: &Repository,
    workdir: &Path,
    status: &WorktreeStatus,
    stash_tracked: bool,
    stash_untracked: bool,
) -> Result<Option<ObjectId>, Error> {
    // Read working-tree content into the ODB before hide_stashed_files overwrites it
    // with the indexed version. Once hidden, the unstaged changes are gone from disk.
    let mut stash_entries: Vec<StashEntry> = Vec::new();
    let mut untracked_entries: Vec<StashEntry> = Vec::new();

    // Always stash partially-staged files (those in both staged and dirty)
    for path in &status.staged {
        if status.dirty.contains(path) {
            let (oid, mode) = read_blob(repo, workdir, path)?;
            stash_entries.push(StashEntry {
                path: path.clone(),
                oid,
                mode,
            });
        }
    }

    // Additionally stash dirty tracked files if requested (dirty \ staged)
    if stash_tracked {
        for path in &status.dirty {
            if !status.staged.contains(path) {
                let (oid, mode) = read_blob(repo, workdir, path)?;
                stash_entries.push(StashEntry {
                    path: path.clone(),
                    oid,
                    mode,
                });
            }
        }
    }

    if stash_untracked {
        for path in &status.untracked {
            let (oid, mode) = read_blob(repo, workdir, path)?;
            untracked_entries.push(StashEntry {
                path: path.clone(),
                oid,
                mode,
            });
        }
    }

    // Staged files gone from the worktree: materialize indexed content to format, delete on restore.
    let staged_absent: Vec<String> = status
        .staged
        .intersection(&status.missing)
        .cloned()
        .collect();

    if stash_entries.is_empty() && untracked_entries.is_empty() && staged_absent.is_empty() {
        return Ok(None);
    }

    let commit_oid = create_stash_commit(repo, &stash_entries, &untracked_entries, &staged_absent)?;
    hide_stashed_files(
        repo,
        workdir,
        &stash_entries,
        &untracked_entries,
        &staged_absent,
    )?;

    Ok(Some(commit_oid))
}

/// Remove our stash entry from the reflog and update/delete refs/stash.
pub fn remove(repo: &Repository, oid: ObjectId) -> Result<(), Error> {
    let reflog_path = repo.common_dir().join("logs/refs/stash");
    if !reflog_path.exists() {
        return Ok(());
    }
    let mut reflog_lock = LockFile::acquire(&reflog_path)?;
    let reflog_file = match fs::File::open(&reflog_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(Error::FileRead {
                path: reflog_path,
                source: e,
            });
        }
    };

    let our_hex = oid.to_hex().to_string();
    let mut last_hex = None;

    // Stream reflog into the lock file, skipping our entry.
    // Each line: "<old_oid> <new_oid> <signature>\t<message>"
    for line in BufReader::new(reflog_file).lines() {
        let line = line.map_err(|e| Error::FileRead {
            path: reflog_path.clone(),
            source: e,
        })?;
        if line.is_empty() {
            continue;
        }
        let new_hex = line.split(' ').nth(1);
        if new_hex != Some(our_hex.as_str()) {
            writeln!(reflog_lock, "{line}").map_err(|e| Error::FileWrite {
                path: reflog_lock.path().to_path_buf(),
                source: e,
            })?;
            last_hex = new_hex.map(str::to_owned);
        }
    }

    if let Some(hex) = last_hex {
        let ref_path = repo.common_dir().join("refs/stash");
        let mut ref_lock = LockFile::acquire(&ref_path)?;
        ref_lock
            .write_all(format!("{hex}\n").as_bytes())
            .map_err(|e| Error::FileWrite {
                path: ref_lock.path().to_path_buf(),
                source: e,
            })?;
        ref_lock.commit()?;
        reflog_lock.commit()?;
    } else {
        // No remaining entries: delete the ref and reflog entirely.
        if let Ok(r) = repo.find_reference("refs/stash") {
            r.delete().map_err(Error::RefDelete)?;
        }
        fs::remove_file(&reflog_path).ok();
    }

    Ok(())
}

/// Restore stashed files to the working tree.
///
/// Restores dirty tracked files by comparing the stash commit tree against HEAD,
/// deletes files in HEAD but absent from the stash tree (worktree deletions, e.g., rename
/// sources), then restores untracked files from parent[2]'s tree.
///
/// Returns the paths of all files written, for `index::refresh_stat`.
pub fn restore(
    repo: &Repository,
    workdir: &Path,
    stash_oid: ObjectId,
) -> Result<Vec<String>, Error> {
    let use_symlinks = repo
        .config_snapshot()
        .boolean("core.symlinks")
        .unwrap_or(cfg!(unix));

    let stash_obj = repo.find_object(stash_oid).map_err(Error::ObjectFind)?;
    let stash_commit = stash_obj.into_commit();
    let parents: Vec<ObjectId> = stash_commit.parent_ids().map(Id::detach).collect();
    let stash_tree_oid = stash_commit
        .tree_id()
        .map_err(|e| Error::TreeDecode(e.into()))?
        .detach();

    // Stash parents: [HEAD, index, untracked?], or [untracked?] on an empty repo (>=2 means HEAD+index).
    let has_head = parents.len() >= 2;
    let head_oid = has_head.then(|| parents[0]);
    let mut restored = restore_stash_files(repo, workdir, head_oid, stash_tree_oid, use_symlinks)?;

    let untracked_idx = if has_head { 2 } else { 0 };
    if let Some(&untracked_oid) = parents.get(untracked_idx) {
        restore_untracked_files(repo, workdir, untracked_oid, &mut restored, use_symlinks)?;
    }

    Ok(restored)
}

/// Read a file's content into the ODB and return its OID + tree entry mode.
fn read_blob(
    repo: &Repository,
    workdir: &Path,
    path: &str,
) -> Result<(ObjectId, EntryMode), Error> {
    let file_path = workdir.join(path);
    let meta = file_path.symlink_metadata().map_err(|e| Error::FileRead {
        path: file_path.clone(),
        source: e,
    })?;

    let is_symlink = meta.file_type().is_symlink();

    let mode = if is_symlink {
        EntryKind::Link.into()
    } else if is_executable(&meta) {
        EntryKind::BlobExecutable.into()
    } else {
        EntryKind::Blob.into()
    };

    let content = if is_symlink {
        let target = fs::read_link(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            target.as_os_str().as_bytes().to_vec()
        }
        #[cfg(not(unix))]
        target.to_string_lossy().into_owned().into_bytes()
    } else {
        fs::read(&file_path).map_err(|e| Error::FileRead {
            path: file_path.clone(),
            source: e,
        })?
    };

    let oid = repo
        .write_blob(&content)
        .map_err(|e| Error::BlobWrite {
            path: path.to_owned(),
            source: e,
        })?
        .detach();

    Ok((oid, mode))
}

/// Build the stash commit and write refs/stash (if HEAD exists).
fn create_stash_commit(
    repo: &Repository,
    stash_entries: &[StashEntry],
    untracked_entries: &[StashEntry],
    staged_absent: &[String],
) -> Result<ObjectId, Error> {
    let committer = repo
        .committer()
        .ok_or(Error::NoIdentity)?
        .map_err(Error::CommitterTime)?
        .to_owned()
        .map_err(Error::CommitterValidation)?;

    let head = repo.head_commit().ok();

    // Build stash tree: HEAD tree (or empty tree) with stashed tracked files overlaid.
    let base_tree = match &head {
        Some(commit) => commit.tree().map_err(Error::TreeDecode)?,
        None => repo.empty_tree(),
    };
    let index = open_index(repo).map_err(Error::IndexRead)?;
    let mut editor = base_tree.edit().map_err(Error::TreeEditInit)?;
    for entry in stash_entries {
        editor
            .upsert(entry.path.as_str(), entry.mode.kind(), entry.oid)
            .map_err(Error::TreeEdit)?;
    }
    // Mirror git's w_tree so git stash pop can recover after a crash.
    for path in staged_absent {
        if base_tree
            .lookup_entry_by_path(Path::new(path))
            .map_err(Error::ObjectFind)?
            .is_some()
        {
            editor.remove(path.as_str()).map_err(Error::TreeEdit)?;
        } else if let Some(entry) =
            index.entry_by_path_and_stage(path.as_bytes().as_bstr(), Stage::Unconflicted)
            && let Some(mode) = entry.mode.to_tree_entry_mode()
        {
            editor
                .upsert(path.as_str(), mode.kind(), entry.id)
                .map_err(Error::TreeEdit)?;
        }
    }
    let stash_tree_oid = editor.write().map_err(Error::TreeEditorWrite)?.detach();

    let mut parents: Vec<ObjectId> = Vec::new();
    if let Some(commit) = &head {
        parents.push(commit.id);

        // parent[1]: index commit - tree is the current staged state.
        // git stash pop uses this as the merge base for staged changes; using HEAD
        // tree instead would produce wrong 3-way merge results for partially-staged files.
        let index_tree_oid = {
            let mut editor = repo.empty_tree().edit().map_err(Error::TreeEditInit)?;
            for entry in index.entries() {
                if entry.flags.stage() != Stage::Unconflicted {
                    continue;
                }
                let path =
                    std::str::from_utf8(entry.path(&index)).map_err(|_| Error::NonUtf8Path)?;
                let Some(mode) = entry.mode.to_tree_entry_mode() else {
                    continue;
                };
                editor
                    .upsert(path, mode.kind(), entry.id)
                    .map_err(Error::TreeEdit)?;
            }
            editor.write().map_err(Error::TreeEditorWrite)?.detach()
        };
        let index_commit_oid = write_commit(
            repo,
            &committer,
            index_tree_oid,
            &[commit.id],
            "stagelint: index on HEAD",
        )?;
        parents.push(index_commit_oid);
    }

    // parent[2]: untracked commit (only when untracked files exist)
    if !untracked_entries.is_empty() {
        let mut editor = repo.empty_tree().edit().map_err(Error::TreeEditInit)?;
        for entry in untracked_entries {
            editor
                .upsert(entry.path.as_str(), entry.mode.kind(), entry.oid)
                .map_err(Error::TreeEdit)?;
        }
        let untracked_tree_oid = editor.write().map_err(Error::TreeEditorWrite)?.detach();
        let untracked_commit_oid =
            write_commit(repo, &committer, untracked_tree_oid, &[], "untracked files")?;
        parents.push(untracked_commit_oid);
    }

    let commit_oid = write_commit(
        repo,
        &committer,
        stash_tree_oid,
        &parents,
        "stagelint automatic backup",
    )?;

    // Skip on empty repos - git stash pop can't handle orphan commits.
    if head.is_some() {
        repo.edit_reference(RefEdit {
            change: Change::Update {
                log: LogChange {
                    mode: RefLog::AndReference,
                    force_create_reflog: true,
                    message: "stagelint automatic backup".into(),
                },
                expected: PreviousValue::Any,
                new: Target::Object(commit_oid),
            },
            name: "refs/stash".try_into().expect("valid ref name"),
            deref: false,
        })
        .map_err(Error::RefWrite)?;
    }

    Ok(commit_oid)
}

fn write_commit(
    repo: &Repository,
    committer: &gix::actor::Signature,
    tree: ObjectId,
    parents: &[ObjectId],
    message: &str,
) -> Result<ObjectId, Error> {
    let commit = Commit {
        tree,
        parents: parents.to_vec().into(),
        author: committer.clone(),
        committer: committer.clone(),
        encoding: None,
        message: message.into(),
        extra_headers: vec![],
    };
    repo.write_object(&commit)
        .map(Id::detach)
        .map_err(Error::CommitWrite)
}

/// Replace stashed files with their correct on-disk representation.
/// For tracked files: write indexed blob content.
/// For untracked files: delete them so they're absent during formatting.
/// For absent staged files: write indexed content so the formatter can process them.
fn hide_stashed_files(
    repo: &Repository,
    workdir: &Path,
    stash_entries: &[StashEntry],
    untracked_entries: &[StashEntry],
    staged_absent: &[String],
) -> Result<(), Error> {
    let use_symlinks = repo
        .config_snapshot()
        .boolean("core.symlinks")
        .unwrap_or(cfg!(unix));
    let index = open_index(repo).map_err(Error::IndexRead)?;

    for entry in stash_entries {
        let path = entry.path.as_str();
        let bpath = path.as_bytes().as_bstr();
        let Some(entry) = index.entry_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };
        let blob = repo.find_object(entry.id).map_err(|e| Error::BlobRead {
            path: path.to_owned(),
            source: e,
        })?;
        let file_path = workdir.join(path);
        let mode = entry
            .mode
            .to_tree_entry_mode()
            .unwrap_or(EntryKind::Blob.into());
        write_to_workdir(&file_path, &blob.data, mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path.clone(),
                source: e,
            }
        })?;
    }

    for entry in untracked_entries {
        let file_path = workdir.join(&entry.path);
        remove_if_exists(&file_path).map_err(|e| Error::FileDelete {
            path: file_path,
            source: e,
        })?;
    }

    // Recreate absent staged files from indexed content so the formatter can run; restore deletes them.
    for path in staged_absent {
        let bpath = path.as_bytes().as_bstr();
        let Some(entry) = index.entry_by_path_and_stage(bpath, Stage::Unconflicted) else {
            continue;
        };
        let blob = repo.find_object(entry.id).map_err(|e| Error::BlobRead {
            path: path.clone(),
            source: e,
        })?;
        let file_path = workdir.join(path);
        let mode = entry
            .mode
            .to_tree_entry_mode()
            .unwrap_or(EntryKind::Blob.into());
        write_to_workdir(&file_path, &blob.data, mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path.clone(),
                source: e,
            }
        })?;
    }

    Ok(())
}

/// Restore stash tree files that differ from HEAD to the working tree.
///
/// Diffs HEAD tree -> stash tree: modified/added entries are restored to disk,
/// deleted entries (rename sources) are removed from disk.
fn restore_stash_files(
    repo: &Repository,
    workdir: &Path,
    head_oid: Option<ObjectId>,
    stash_tree_oid: ObjectId,
    use_symlinks: bool,
) -> Result<Vec<String>, Error> {
    let head_tree = if let Some(oid) = head_oid {
        let tree_oid = repo
            .find_object(oid)
            .map_err(Error::ObjectFind)?
            .into_commit()
            .tree_id()
            .map_err(|e| Error::TreeDecode(e.into()))?
            .detach();
        repo.find_object(tree_oid)
            .map_err(Error::ObjectFind)?
            .into_tree()
    } else {
        repo.empty_tree()
    };
    let stash_tree = repo
        .find_object(stash_tree_oid)
        .map_err(Error::ObjectFind)?
        .into_tree();

    let mut restored = Vec::new();
    head_tree
        .changes()
        .map_err(|e| Error::TreeDiff(Box::new(e)))?
        .options(|o| {
            o.track_rewrites(None);
        })
        .for_each_to_obtain_tree(&stash_tree, |change| -> Result<_, Error> {
            use gix::object::tree::diff::Change;
            match change {
                Change::Modification {
                    location,
                    entry_mode,
                    id,
                    ..
                }
                | Change::Addition {
                    location,
                    entry_mode,
                    id,
                    ..
                } => {
                    if entry_mode.is_tree() {
                        return Ok(std::ops::ControlFlow::Continue(()));
                    }
                    let path = location.to_str().map_err(|_| Error::NonUtf8Path)?;
                    let blob = repo.find_object(id.detach()).map_err(Error::ObjectFind)?;
                    let file_path = workdir.join(path);
                    write_to_workdir(&file_path, &blob.data, entry_mode, use_symlinks).map_err(
                        |e| Error::FileWrite {
                            path: file_path,
                            source: e,
                        },
                    )?;
                    restored.push(path.to_owned());
                }
                Change::Deletion { location, .. } => {
                    let path = location.to_str().map_err(|_| Error::NonUtf8Path)?;
                    let file_path = workdir.join(path);
                    remove_if_exists(&file_path).map_err(|e| Error::FileDelete {
                        path: file_path,
                        source: e,
                    })?;
                }
                Change::Rewrite { .. } => {}
            }
            Ok(std::ops::ControlFlow::Continue(()))
        })
        .map_err(|e| Error::TreeDiff(Box::new(e)))?;

    Ok(restored)
}

/// Restore untracked files stored in the stash's third-parent commit.
fn restore_untracked_files(
    repo: &Repository,
    workdir: &Path,
    untracked_commit_oid: ObjectId,
    restored: &mut Vec<String>,
    use_symlinks: bool,
) -> Result<(), Error> {
    let untracked_tree_oid = repo
        .find_object(untracked_commit_oid)
        .map_err(Error::ObjectFind)?
        .into_commit()
        .tree_id()
        .map_err(|e| Error::TreeDecode(e.into()))?
        .detach();
    let untracked_tree = repo
        .find_object(untracked_tree_oid)
        .map_err(Error::ObjectFind)?
        .into_tree();
    for entry in untracked_tree
        .traverse()
        .breadthfirst
        .files()
        .map_err(|e| Error::TreeDiff(Box::new(e)))?
    {
        if entry.mode.is_tree() {
            continue;
        }
        let path = entry.filepath.to_str().map_err(|_| Error::NonUtf8Path)?;
        let blob = repo.find_object(entry.oid).map_err(Error::ObjectFind)?;
        let file_path = workdir.join(path);
        write_to_workdir(&file_path, &blob.data, entry.mode, use_symlinks).map_err(|e| {
            Error::FileWrite {
                path: file_path.clone(),
                source: e,
            }
        })?;
        restored.push(path.to_owned());
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &fs::Metadata) -> bool {
    false
}

fn write_to_workdir(
    file_path: &Path,
    blob: &[u8],
    mode: EntryMode,
    use_symlinks: bool,
) -> Result<(), std::io::Error> {
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }

    if mode.is_link() && use_symlinks {
        remove_if_exists(file_path)?;
        create_symlink(blob, file_path)?;
    } else {
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let want_exec = mode.kind() == EntryKind::BlobExecutable;
            let mode_ok = file_path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_file() && is_executable(&m) == want_exec);
            if mode_ok {
                fs::write(file_path, blob)?;
            } else {
                // Recreate with a broad mode so the kernel applies the umask.
                remove_if_exists(file_path)?;
                let create_mode = if want_exec { 0o777 } else { 0o666 };
                fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(create_mode)
                    .open(file_path)?
                    .write_all(blob)?;
            }
        }
        #[cfg(not(unix))]
        {
            // Unlink an existing symlink first; write() would follow it and corrupt the target.
            if file_path
                .symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
            {
                remove_if_exists(file_path)?;
            }
            fs::write(file_path, blob)?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn create_symlink(blob: &[u8], link: &Path) -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(blob), link)
}

#[cfg(windows)]
fn create_symlink(blob: &[u8], link: &Path) -> std::io::Result<()> {
    let target = std::str::from_utf8(blob)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Resolve target relative to symlink's parent to determine whether it's a directory.
    // If the target doesn't exist yet, default to symlink_file - matches git's own behavior.
    let is_dir = link
        .parent()
        .map_or_else(|| Path::new(target).to_path_buf(), |p| p.join(target))
        .is_dir();
    if is_dir {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    }
}

fn remove_if_exists(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(windows)]
    {
        // On Windows, remove_file fails on directory symlinks - use remove_dir for those.
        match path.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
            Ok(_) if fs::metadata(path).is_ok_and(|m| m.is_dir()) => {
                return fs::remove_dir(path);
            }
            Ok(_) => {}
        }
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
