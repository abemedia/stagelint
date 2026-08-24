use std::fs;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use gix::bstr::{BString, ByteSlice};
use gix::discover::upwards;

use crate::report::{Reporter, Status};

const BEGIN: &str = "# stagelint begin";
const END: &str = "# stagelint end";

pub fn install(force: bool, root: &Reporter) -> Result<()> {
    let repo = match gix::discover(std::env::current_dir()?) {
        Ok(repo) => repo,
        Err(gix::discover::Error::Discover(upwards::Error::NoGitRepository { path })) => {
            root.add(format!(
                "No git repository in {}; skipping hook installation",
                path.display()
            ))
            .status(Status::Warn);
            return Ok(());
        }
        Err(e) => return Err(e.into()),
    };
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
    fs::create_dir_all(&hooks_dir)
        .with_context(|| format!("failed to create {}", hooks_dir.display()))?;
    let hook_path = hooks_dir.join("pre-commit");

    let exe = std::env::current_exe()?;
    let command = exe
        .strip_prefix(&workdir)
        .map(Path::to_owned)
        .ok()
        .or_else(|| {
            let (exe, root) = (exe.canonicalize().ok()?, workdir.canonicalize().ok()?);
            exe.strip_prefix(root).map(Path::to_owned).ok()
        })
        .map(|rel| Path::new(".").join(rel))
        .or_else(|| which::which("stagelint").ok())
        .unwrap_or(exe);
    let section = section(&command);

    let existing = fs::symlink_metadata(&hook_path);
    let previous = existing
        .as_ref()
        .is_ok_and(fs::Metadata::is_file)
        .then(|| fs::read(&hook_path))
        .transpose()
        .with_context(|| format!("failed to read {}", hook_path.display()))?;

    let replaced = previous
        .as_deref()
        .and_then(|hook| replace_section(hook, &section));
    let updated = match replaced {
        Some(hook) => hook,
        None if !force && existing.is_ok() => bail!(
            "pre-commit hook already exists at {}\nUse --force to overwrite",
            hook_path.display()
        ),
        None => [b"#!/bin/sh\n".as_slice(), &section].concat().into(),
    };

    if previous.as_deref() != Some(updated.as_slice()) {
        if let Err(e) = fs::remove_file(&hook_path)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(e).with_context(|| format!("failed to remove {}", hook_path.display()));
        }
        fs::write(&hook_path, &updated)
            .with_context(|| format!("failed to write {}", hook_path.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to make {} executable", hook_path.display()))?;
    }

    root.add(format!(
        "Installed pre-commit hook at {}",
        hook_path.display()
    ))
    .status(Status::Done);

    Ok(())
}

/// Replace our section, keeping whatever the user put around it. `None` when unmarked.
fn replace_section(hook: &[u8], section: &[u8]) -> Option<BString> {
    let begin = hook.find(BEGIN)?;
    let end = hook.find(END).filter(|end| begin < *end)? + END.len();
    let mut out = BString::from(&hook[..begin]);
    out.extend_from_slice(section.trim_end());
    out.extend_from_slice(&hook[end..]);
    Some(out)
}

fn section(command: &Path) -> BString {
    let command = gix::path::into_bstr(command);
    // git's bundled sh reads backslashes as escapes.
    #[cfg(windows)]
    let command = command.replace(b"\\", b"/");

    // Not `exec`: user lines after ours must run.
    let mut section = BString::from(BEGIN);
    section.extend_from_slice(b"\n'");
    section.extend_from_slice(&command.replace(b"'", br"'\''"));
    section.extend_from_slice(b"' \"$@\" || exit $?\n");
    section.extend_from_slice(END.as_bytes());
    section.push(b'\n');
    section
}

#[cfg(test)]
mod tests {
    use super::{BEGIN, END, replace_section, section};
    use std::path::Path;

    fn command_of(command: &str) -> String {
        section(Path::new(command))
            .to_string()
            .lines()
            .nth(1)
            .expect("command line")
            .strip_suffix(r#" "$@" || exit $?"#)
            .expect("invocation suffix")
            .to_owned()
    }

    /// sh ends a single-quoted word at the first quote; the escape closes and reopens around it.
    #[test]
    fn apostrophe_is_escaped() {
        assert_eq!(
            command_of("/home/o'brien/bin/stagelint"),
            r"'/home/o'\''brien/bin/stagelint'"
        );
    }

    #[test]
    fn relative_command_is_quoted_whole() {
        assert_eq!(
            command_of("./node_modules/@stagelint/darwin-arm64/stagelint"),
            "'./node_modules/@stagelint/darwin-arm64/stagelint'"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_path_survives() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let command = Path::new(OsStr::from_bytes(b"/tmp/dir-\xff/stagelint"));
        let section = section(command);
        assert!(
            section.contains(&b'\xff'),
            "the raw byte should reach the hook: {section:?}"
        );
    }

    #[test]
    fn replaces_only_the_marked_section() {
        let section = format!("{BEGIN}\nNEW\n{END}\n");
        let hook = format!("#!/bin/sh\nbefore\n{BEGIN}\nold\n{END}\nafter\n");
        let replaced = replace_section(hook.as_bytes(), section.as_bytes()).expect("markers found");
        assert_eq!(replaced, format!("#!/bin/sh\nbefore\n{section}after\n"));
    }

    #[test]
    fn replacement_is_idempotent() {
        let section = format!("{BEGIN}\nrun\n{END}\n");
        let hook = format!("#!/bin/sh\n{section}trailing\n");
        let once = replace_section(hook.as_bytes(), section.as_bytes()).expect("markers found");
        let twice = replace_section(&once, section.as_bytes()).expect("markers found");
        assert_eq!(once, twice);
        assert_eq!(once, hook.as_str());
    }

    #[test]
    fn rejects_hooks_without_usable_markers() {
        assert!(replace_section(b"#!/bin/sh\nsomething\n", b"NEW\n").is_none());
        assert!(replace_section(format!("{BEGIN}\nno end\n").as_bytes(), b"NEW\n").is_none());
        assert!(replace_section(format!("{END}\n{BEGIN}\n").as_bytes(), b"NEW\n").is_none());
    }
}
