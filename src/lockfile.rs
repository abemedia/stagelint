use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(
        "{path} is locked by another process; if no git process is running, delete it manually to recover"
    )]
    AlreadyHeld { path: PathBuf },
    #[error("failed to acquire lock for {path}")]
    Acquire {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to commit lock for {path}")]
    Commit {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// RAII exclusive lock file following git's lockfile convention.
///
/// `acquire(target)` creates `target.lock` with `O_CREAT|O_EXCL`.
/// Write new content via the `Write` impl.
/// `commit()` renames it back to `target`, atomically publishing the change.
/// If dropped without committing, the lock file is deleted automatically.
pub(crate) struct LockFile {
    lock_path: Option<PathBuf>, // None after a successful commit (prevents Drop cleanup)
    target_path: PathBuf,
    file: Option<fs::File>,
}

impl LockFile {
    /// Lock `target` by creating `target.lock` exclusively.
    ///
    /// Note: symlink resolution is intentionally omitted. A general-purpose
    /// implementation would resolve symlinks first so the lock file lands next
    /// to the real file rather than the link.
    pub fn acquire(target: &Path) -> Result<Self, Error> {
        let lock_path = PathBuf::from({
            let mut s = std::ffi::OsString::from(target);
            s.push(".lock");
            s
        });
        let file = fs::File::options()
            .write(true)
            .create_new(true)
            .open(&lock_path)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    Error::AlreadyHeld {
                        path: lock_path.clone(),
                    }
                } else {
                    Error::Acquire {
                        path: lock_path.clone(),
                        source: e,
                    }
                }
            })?;
        Ok(Self {
            lock_path: Some(lock_path),
            target_path: target.to_path_buf(),
            file: Some(file),
        })
    }

    /// Path of the lock file (for use in write-error messages).
    pub fn path(&self) -> &Path {
        self.lock_path.as_deref().unwrap()
    }

    /// Atomically rename the lock file to its target, committing the change.
    /// On failure the lock file is left in place and Drop will clean it up.
    pub fn commit(mut self) -> Result<(), Error> {
        drop(self.file.take()); // close before rename
        let path = self.lock_path.take().unwrap();
        fs::rename(&path, &self.target_path).map_err(|e| {
            self.lock_path = Some(path); // restore so Drop cleans up on failure
            Error::Commit {
                path: self.target_path.clone(),
                source: e,
            }
        })
    }
}

impl Write for LockFile {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // unwrap is safe: file is only taken in commit(), which consumes self,
        // and in drop(), after which no methods can be called.
        self.file.as_mut().unwrap().write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // File writes go directly to the OS; this is a no-op but required by Write.
        self.file.as_mut().unwrap().flush()
    }
}

impl Drop for LockFile {
    fn drop(&mut self) {
        if let Some(path) = self.lock_path.take() {
            drop(self.file.take()); // close before delete
            fs::remove_file(path).ok();
        }
    }
}
