mod helpers;

use helpers::*;
use serde_json::json;
use std::fs;
use std::process::{Command, Stdio};
use std::time::Duration;

// Core

/// An empty scope is a success, whatever the source selects.
#[test]
fn empty_scope_succeeds() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    assert_success(repo.stagelint(&[]));
    assert_success(repo.stagelint(&["--unstaged"]));
    assert_success(repo.stagelint(&["--files", "gone.txt"]));
    assert_success(repo.stagelint(&["--diff", "HEAD...HEAD"]));
}

/// The linter's output is committed to the index and working tree.
#[test]
fn formats_staged_file() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("hello.txt", "hello world\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO WORLD\n");
    assert_eq!(repo.read_file("hello.txt"), "HELLO WORLD\n");
}

/// Running from a repo subdirectory behaves identically to running from the root.
#[test]
fn runs_from_subdirectory() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);

    let sub = repo.root.join("sub/dir");
    fs::create_dir_all(&sub).expect("mkdir");

    let mut cmd = repo.stagelint_cmd();
    let output = cmd.current_dir(&sub).output().expect("run stagelint");
    assert!(
        output.status.success(),
        "stagelint failed from a subdirectory: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(repo.git(&["show", ":file.txt"]), "HELLO\n");
}

/// A failing run rolls back every linter side-effect while leaving the user's changes alone.
#[test]
fn failure_rolls_back_all_side_effects() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({
        "*.txt": [
            "sh -c 'echo SIDE EFFECT > clean_modified.txt'",
            "sh -c 'rm clean_deleted.txt dirty_deleted.txt'",
            "sh -c 'echo JUNK >> dirty_modified.txt'",
            "sh -c 'echo RESURRECTED > user_deleted.txt'",
            "sh -c 'rm clean_link.txt; ln -s clean_modified.txt clean_link.txt'",
            "false"
        ]
    }));
    repo.git(&["config", "core.symlinks", "true"]);

    repo.write_file("clean_modified.txt", "clean\n");
    repo.write_file("clean_deleted.txt", "content\n");
    symlink_file("clean_deleted.txt", &repo.root.join("clean_link.txt")).expect("symlink");
    repo.write_file("dirty_modified.txt", "v0\n");
    repo.write_file("dirty_deleted.txt", "v0\n");
    repo.write_file("user_dirty.txt", "v0\n");
    repo.write_file("user_deleted.txt", "gone\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add files"]);
    repo.write_file("dirty_modified.txt", "v0\nedits\n");
    repo.write_file("dirty_deleted.txt", "v0\nedits\n");
    repo.write_file("user_dirty.txt", "v0\nuser edits\n");
    fs::remove_file(repo.root.join("user_deleted.txt")).expect("delete user file");

    repo.write_file("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);

    assert_failure(repo.stagelint(&[]));

    assert_eq!(
        repo.read_file("clean_modified.txt"),
        "clean\n",
        "side-effect on a clean tracked file should be reverted"
    );
    assert_eq!(
        repo.read_file("clean_deleted.txt"),
        "content\n",
        "clean tracked file deleted by the linter should be restored"
    );
    assert_eq!(
        repo.read_file("dirty_modified.txt"),
        "v0\nedits\n",
        "contaminated dirty file should return to its pre-run bytes"
    );
    assert_eq!(
        repo.read_file("dirty_deleted.txt"),
        "v0\nedits\n",
        "unstaged edits must survive the linter deleting the file"
    );
    assert_eq!(repo.read_file("user_dirty.txt"), "v0\nuser edits\n");
    assert!(
        !repo.root.join("user_deleted.txt").exists(),
        "the user's own deletion must not be resurrected"
    );
    let link = fs::read_link(repo.root.join("clean_link.txt")).expect("readlink");
    assert_eq!(
        link.to_str().unwrap(),
        "clean_deleted.txt",
        "a repointed clean tracked symlink should be reverted, still as a symlink"
    );
}

/// A linter that modifies a file that is not staged: the change must not reach the index.
#[test]
fn unstaged_file_changes_not_indexed() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in *.txt; do echo MODIFIED > \"$f\"; done'"
    }));

    repo.write_file("staged.txt", "original\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("unstaged.txt", "should not change in index\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":staged.txt"]), "MODIFIED\n");

    let ls = repo.git(&["ls-files", "unstaged.txt"]);
    assert!(
        ls.is_empty(),
        "unstaged.txt should not be committed to the index, got: {ls}"
    );
}

/// A linter that deletes the file it was given: the deletion is staged, as `git add` would.
#[test]
fn linter_deletes_staged_file() {
    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'rm \"$@\"' _"}));

    repo.write_file("doomed.txt", "goodbye\n");
    repo.git(&["add", "doomed.txt"]);

    assert_success(repo.stagelint(&[]));

    let show = repo.git_cmd(&["show", ":doomed.txt"]);
    assert!(
        !show.status.success(),
        "the deletion should be staged, not the stale blob"
    );
    assert!(!repo.root.join("doomed.txt").exists());
}

/// A staged file deleted by the linter stays deleted under `--stash tracked`.
#[test]
fn linter_deleted_staged_file_not_resurrected() {
    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'rm \"$@\"' _"}));

    repo.write_file("victim.txt", "v1\n");
    repo.git(&["add", "victim.txt"]);
    repo.git(&["commit", "-m", "add victim"]);
    repo.write_file("victim.txt", "v2\n");
    repo.git(&["add", "victim.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    let show = repo.git_cmd(&["show", ":victim.txt"]);
    assert!(!show.status.success(), "the deletion should be staged");
    assert!(
        !repo.root.join("victim.txt").exists(),
        "the deleted file must not be resurrected"
    );
}

/// A partially staged file deleted by the linter: deletion staged, unstaged content preserved.
#[test]
fn linter_deleted_partial_stage_preserves_dirty() {
    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'rm \"$@\"' _"}));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "staged\nunstaged\n");

    assert_success(repo.stagelint(&[]));

    let show = repo.git_cmd(&["show", ":file.txt"]);
    assert!(!show.status.success(), "the deletion should be staged");
    assert_eq!(
        repo.read_file("file.txt"),
        "staged\nunstaged\n",
        "unstaged content must be restored from the stash"
    );
}

/// A linter that rewrites a file and sets its executable bit: both are staged, as `git add` would.
#[cfg(unix)]
#[test]
fn linter_mode_and_content_change_staged() {
    let repo = TestRepo::new(&json!({
        "*.sh": "sh -c 'for f in \"$@\"; do tr a-z A-Z < \"$f\" > \"$f.t\" && mv \"$f.t\" \"$f\" && chmod +x \"$f\"; done' _"
    }));

    repo.write_file("run.sh", "echo hi\n");
    repo.git(&["add", "run.sh"]);

    assert_success(repo.stagelint(&[]));

    let ls = repo.git(&["ls-files", "-s", "run.sh"]);
    assert!(
        ls.starts_with("100755"),
        "the executable bit should be staged, got: {ls}"
    );
    assert_eq!(repo.git(&["show", ":run.sh"]), "ECHO HI\n");
}

/// A linter that only flips the executable bit: the mode change is staged, as `git add` would.
#[cfg(unix)]
#[test]
fn linter_mode_only_change_staged() {
    let repo = TestRepo::new(&json!({"*.sh": "sh -c 'chmod +x \"$@\"' _"}));

    repo.write_file("run.sh", "echo hi\n");
    repo.git(&["add", "run.sh"]);

    assert_success(repo.stagelint(&[]));

    let ls = repo.git(&["ls-files", "-s", "run.sh"]);
    assert!(
        ls.starts_with("100755"),
        "the executable bit should be staged, got: {ls}"
    );
}

/// A chmod-only change is staged even when `core.trustctime=false` makes the stat look clean.
#[cfg(unix)]
#[test]
fn trustctime_disabled_stages_mode_only_change() {
    let repo = TestRepo::new(&json!({"*.sh": "sh -c 'chmod +x \"$@\"' _"}));
    repo.git(&["config", "core.trustctime", "false"]);

    repo.write_file("run.sh", "echo hi\n");
    // Backdate mtime so the entry is not racy and the stat shortcut is actually taken.
    let past = std::time::SystemTime::now() - Duration::from_secs(10);
    fs::File::options()
        .write(true)
        .open(repo.root.join("run.sh"))
        .unwrap()
        .set_modified(past)
        .unwrap();
    repo.git(&["add", "run.sh"]);

    assert_success(repo.stagelint(&[]));

    let ls = repo.git(&["ls-files", "-s", "run.sh"]);
    assert!(
        ls.starts_with("100755"),
        "the executable bit should be staged, got: {ls}"
    );
}

/// With `core.fileMode=false`, executable-bit changes are not staged, as `git add` would.
#[cfg(unix)]
#[test]
fn filemode_disabled_ignores_mode_change() {
    let repo = TestRepo::new(&json!({"*.sh": "sh -c 'chmod +x \"$@\"' _"}));
    repo.git(&["config", "core.fileMode", "false"]);

    repo.write_file("run.sh", "echo hi\n");
    repo.git(&["add", "run.sh"]);

    assert_success(repo.stagelint(&[]));

    let ls = repo.git(&["ls-files", "-s", "run.sh"]);
    assert!(
        ls.starts_with("100644"),
        "the mode must stay 100644 with core.fileMode=false, got: {ls}"
    );
}

/// A successful run leaves unrelated dirty files dirty, in content and in `git status`.
#[test]
fn success_preserves_dirty_file_status() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("dirty.txt", "v0\n");
    repo.git(&["add", "dirty.txt"]);
    repo.git(&["commit", "-m", "add dirty"]);
    repo.write_file("dirty.txt", "v0\nedits\n");

    repo.write_file("staged.txt", "hello\n");
    repo.git(&["add", "staged.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.read_file("dirty.txt"), "v0\nedits\n");
    let status = repo.git(&["status", "--short"]);
    assert!(
        status.contains(" M dirty.txt"),
        "dirty file should still show as modified, got: {status}"
    );
}

/// Running stagelint twice in sequence leaves clean state each time.
#[test]
fn sequential_runs_leave_clean_state() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("a.txt", "hello\n");
    repo.git(&["add", "a.txt"]);
    assert_success(repo.stagelint(&[]));
    assert_eq!(repo.git(&["show", ":a.txt"]), "HELLO\n");

    repo.git(&["commit", "-m", "first"]);

    repo.write_file("b.txt", "world\n");
    repo.git(&["add", "b.txt"]);
    assert_success(repo.stagelint(&[]));
    assert_eq!(repo.git(&["show", ":b.txt"]), "WORLD\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.is_empty(),
        "no stash entries should remain, got: {stash_list}"
    );
}

/// Staged filenames containing Unicode and spaces are handled correctly.
#[test]
fn handles_unicode_and_space_filenames() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("привет.txt", "hello\n");
    repo.git(&["add", "привет.txt"]);
    repo.write_file("hello world.txt", "hello\n");
    repo.git(&["add", "hello world.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":привет.txt"]), "HELLO\n");
    assert_eq!(repo.git(&["show", ":hello world.txt"]), "HELLO\n");
}

/// A tracked filename that is not valid UTF-8 does not block runs that never touch it.
/// git stores paths as bytes; such names are legal and appear in repos created on other systems.
#[cfg(unix)]
#[test]
fn non_utf8_bystander_does_not_block_run() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStrExt;

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("seed.txt", "legacy\n");
    repo.git(&["add", "seed.txt"]);
    let oid = repo.git(&["rev-parse", ":seed.txt"]).trim().to_owned();

    let mut latin1 = OsString::from("caf");
    latin1.push(std::ffi::OsStr::from_bytes(&[0xE9]));
    latin1.push(".txt");
    let staged = Command::new("git")
        .args(["update-index", "--add", "--cacheinfo", "100644", &oid])
        .arg(&latin1)
        .current_dir(&repo.root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", repo.root.join(".git/no-global-config"))
        .status()
        .expect("git update-index");
    assert!(staged.success(), "planting the latin-1 index entry failed");
    repo.git(&["commit", "-m", "latin-1 bystander"]);

    repo.write_file("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\n");
}

/// stagelint fails immediately when invoked outside a git repository.
#[test]
fn fails_outside_git_repo() {
    let non_git_dir = tempfile::Builder::new()
        .prefix("stagelint-non-git")
        .tempdir()
        .expect("create temp dir");

    let output = Command::new(stagelint_exe())
        .current_dir(non_git_dir.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            non_git_dir.path().join("no-global-config"),
        )
        // Discovery must not escape the temp dir and find an enclosing repository.
        .env("GIT_CEILING_DIRECTORIES", non_git_dir.path())
        .output()
        .expect("run stagelint");

    assert!(
        !output.status.success(),
        "stagelint should fail when not in a git directory"
    );
}

/// Untracked files are left untouched by default.
#[test]
fn ignores_untracked_files() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("staged.txt", "hello\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("untracked.txt", "untracked content\n");
    repo.write_file("another-untracked.txt", "more untracked\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":staged.txt"]), "HELLO\n");
    assert_eq!(repo.read_file("untracked.txt"), "untracked content\n");
    assert_eq!(repo.read_file("another-untracked.txt"), "more untracked\n");
}

/// On an empty repo, a fully staged file is formatted correctly.
#[test]
fn empty_repo_fully_staged() {
    let repo = TestRepo::empty();
    let config_str = serde_json::to_string(&json!({"*.txt": UPPERCASE})).unwrap();
    repo.write_file(".stagelint.json", &config_str);
    repo.git(&["add", ".stagelint.json"]);

    repo.write_file("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\n");
}

/// On an empty repo, partial staging works correctly.
#[test]
fn empty_repo_partially_staged() {
    let repo = TestRepo::empty();
    let config_str =
        serde_json::to_string(&json!({"*.txt": replace("line2", "FORMATTED")})).unwrap();
    repo.write_file(".stagelint.json", &config_str);
    repo.git(&["add", ".stagelint.json"]);

    repo.write_file("file.txt", "line1\nline2\nline3\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "line1\nline2\nline3\nline4\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "line1\nFORMATTED\nline3\n"
    );
    assert_eq!(
        repo.read_file("file.txt"),
        "line1\nFORMATTED\nline3\nline4\n"
    );
}

/// On an empty repo, a failed run still restores index and working tree cleanly.
#[test]
fn empty_repo_failure_restores() {
    let repo = TestRepo::empty();
    let config_str = serde_json::to_string(&json!({"*.txt": "false"})).unwrap();
    repo.write_file(".stagelint.json", &config_str);
    repo.git(&["add", ".stagelint.json"]);

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree\n");

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.read_file("file.txt"), "working tree\n");
    assert_eq!(repo.git(&["show", ":file.txt"]), "staged\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.is_empty(),
        "stash ref should be dropped after failure: {stash_list}"
    );
}

/// On an empty repo, `--stash untracked` restores untracked files instead of losing them.
#[test]
fn empty_repo_stash_untracked_restores() {
    let repo = TestRepo::empty();
    let config_str = serde_json::to_string(&json!({"*.txt": UPPERCASE})).unwrap();
    repo.write_file(".stagelint.json", &config_str);
    repo.git(&["add", ".stagelint.json"]);

    repo.write_file("staged.txt", "hello\n");
    repo.git(&["add", "staged.txt"]);

    repo.write_file("untracked.txt", "keep me\n");

    assert_success(repo.stagelint(&["--stash", "untracked"]));

    assert_eq!(repo.read_file("untracked.txt"), "keep me\n");
    assert_eq!(repo.git(&["show", ":staged.txt"]), "HELLO\n");
}

/// SIGTERM mid-run cancels the linter and restores the repo with no stash left behind.
/// SIGINT (Ctrl+C) shares the same handler, so this covers both.
#[test]
#[cfg(unix)]
fn sigterm_restores_repo() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree\n");

    let child = repo.stagelint(&[]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));

    assert_eq!(
        repo.read_file("file.txt"),
        "staged\n",
        "working tree should show only staged content while stash is active"
    );

    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("send SIGTERM");

    let output = wait_bounded(child);
    assert_eq!(
        output.status.code(),
        Some(1),
        "SIGTERM should trigger a controlled exit, not a signal-kill"
    );

    assert_eq!(repo.git(&["show", ":file.txt"]), "staged\n");
    assert_eq!(repo.read_file("file.txt"), "working tree\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.is_empty(),
        "no stash entries should remain after interrupt, got: {stash_list}"
    );
}

/// Ctrl+C restores the repo even when a pipe-holding background child outlives the leader.
#[test]
#[cfg(unix)]
fn ctrl_c_kills_background_pipe_holder() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'sleep 30 & touch .git/linter-1; exit 0'"
    }));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree\n");

    let child = repo.stagelint(&[]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    // Let the leader's `exit 0` land so only the backgrounded sleep holds the pipes.
    std::thread::sleep(Duration::from_millis(500));

    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");

    let output = wait_bounded(child);
    assert_eq!(output.status.code(), Some(1), "expected a controlled exit");

    assert_eq!(repo.read_file("file.txt"), "working tree\n");
    assert!(repo.git(&["stash", "list"]).is_empty());
}

// Stash

/// On linter failure, index and working tree are restored and the stash ref is dropped.
#[test]
fn linter_failure_restores_state_and_drops_stash() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree\n");

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.read_file("file.txt"), "working tree\n");
    assert_eq!(repo.git(&["show", ":file.txt"]), "staged\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        !stash_list.contains("stagelint"),
        "stash ref should be dropped after failure: {stash_list}"
    );
}

/// `--stash tracked` hides an unstaged deletion: present during the run, deleted again after.
#[test]
fn stash_tracked_hides_unstaged_deletion() {
    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'test -e victim.txt'"}));

    repo.write_file("victim.txt", "tracked\n");
    repo.git(&["add", "victim.txt"]);
    repo.git(&["commit", "-m", "add victim"]);

    repo.write_file("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);

    fs::remove_file(repo.root.join("victim.txt")).expect("delete victim");

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert!(
        !repo.root.join("victim.txt").exists(),
        "deletion should be restored after the run"
    );
    assert_eq!(repo.git(&["show", ":victim.txt"]), "tracked\n");
    assert!(repo.git(&["stash", "list"]).is_empty());
}

/// `--stash tracked` with no dirty files: succeeds and leaves no stash entries.
#[test]
fn stash_tracked_noop_when_clean() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.is_empty(),
        "no stash entries should exist, got: {stash_list}"
    );
}

/// Sparse-checkout entries are absent on purpose and must not be materialized.
#[test]
fn sparse_checkout_files_not_materialized() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("keep.txt", "hello\n");
    repo.write_file("excluded/gone.txt", "sparse\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add files"]);

    // Cone mode with no directories keeps only root files on disk.
    repo.git(&["sparse-checkout", "set"]);
    assert!(
        !repo.root.join("excluded/gone.txt").exists(),
        "sparse-checkout setup should remove the excluded file"
    );

    repo.write_file("keep.txt", "updated\n");
    repo.git(&["add", "keep.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert!(
        !repo.root.join("excluded/gone.txt").exists(),
        "sparse-excluded files must not be materialized"
    );
    assert!(!repo.git(&["ls-files", "excluded/gone.txt"]).is_empty());
}

/// Skip-worktree entries are absent on purpose and must not be staged as deletions.
#[test]
fn sparse_checkout_staged_path_not_deleted() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("keep.txt", "hello\n");
    repo.write_file("excluded/gone.txt", "sparse\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add files"]);

    repo.git(&["sparse-checkout", "set"]);

    // Mimics a merge: an excluded path is not on disk, so `git add` cannot stage it.
    repo.write_file("keep.txt", "changed\n");
    repo.git(&["add", "keep.txt"]);
    let oid = repo.git(&["rev-parse", ":keep.txt"]).trim().to_owned();
    repo.git(&[
        "update-index",
        "--cacheinfo",
        "100644",
        &oid,
        "excluded/gone.txt",
    ]);
    // --cacheinfo rewrites the entry and clears the flag sparse-checkout set on it.
    repo.git(&["update-index", "--skip-worktree", "excluded/gone.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&[
            "diff",
            "--cached",
            "--name-status",
            "--",
            "excluded/gone.txt"
        ]),
        "M\texcluded/gone.txt\n",
        "the sparse path must stay modified, not become a deletion"
    );
    assert_eq!(repo.git(&["show", ":keep.txt"]), "CHANGED\n");
}

/// `--stash tracked` restores clean files the linter touched, leaving only the partial file dirty.
#[test]
fn stash_tracked_restores_clean_files() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in *.txt; do echo MODIFIED > \"$f\"; done'"
    }));

    // A committed file the linter will touch as a side effect.
    repo.write_file("committed.txt", "hello\n");
    repo.git(&["add", "committed.txt"]);
    repo.git(&["commit", "-m", "add committed"]);

    // A partially staged file: exercises the stash save/restore path.
    repo.write_file("partial.txt", "world\n");
    repo.git(&["add", "partial.txt"]);
    repo.write_file("partial.txt", "world\nextra unstaged\n");

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(repo.read_file("committed.txt"), "hello\n");
    let diff = repo.git(&["diff", "--name-only"]);
    assert_eq!(
        diff, "partial.txt\n",
        "only the partially-staged file should be dirty, got: {diff:?}"
    );
}

/// `--stash tracked` hides an intent-to-add file to empty, restores it, and never stages it.
#[test]
fn stash_tracked_intent_to_add_round_trip() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'cat ita.txt > seen.txt'", "pass_filenames": false}
    }));

    repo.write_file("staged.txt", "hello\n");
    repo.git(&["add", "staged.txt"]);

    repo.write_file("ita.txt", "precious\n");
    repo.git(&["add", "-N", "ita.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(repo.read_file("ita.txt"), "precious\n");
    assert_eq!(
        repo.read_file("seen.txt"),
        "",
        "the run must see the staged (empty) content"
    );
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]),
        "staged.txt\n",
        "the next commit must not include the intent-to-add file"
    );
}

/// `--stash untracked` hides dirty tracked and untracked files from the linter.
#[test]
fn stash_untracked_hides_dirty_and_untracked() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'git status --porcelain > manifest.txt'", "pass_filenames": false}
    }));

    repo.write_file("tracked.txt", "committed\n");
    repo.git(&["add", "tracked.txt"]);
    repo.git(&["commit", "-m", "add tracked"]);
    repo.write_file("tracked.txt", "dirty tracked\n");

    repo.write_file("staged.txt", "staged content\n");
    repo.git(&["add", "staged.txt"]);

    repo.write_file("untracked.txt", "untracked content\n");

    assert_success(repo.stagelint(&["--stash", "untracked"]));

    let manifest = repo.read_file("manifest.txt");
    assert!(
        !manifest.contains("tracked.txt"),
        "dirty tracked file should be hidden, manifest: {manifest}"
    );
    assert!(
        !manifest.contains("untracked.txt"),
        "untracked file should be hidden, manifest: {manifest}"
    );

    assert_eq!(repo.read_file("tracked.txt"), "dirty tracked\n");
    assert_eq!(repo.read_file("untracked.txt"), "untracked content\n");
}

/// On failure with `--stash untracked`, the stash is cleaned up and all files are restored.
#[test]
fn stash_untracked_failure_restores() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("partial.txt", "original\n");
    repo.git(&["add", "partial.txt"]);
    repo.git(&["commit", "-m", "add partial"]);

    repo.write_file("partial.txt", "staged\n");
    repo.git(&["add", "partial.txt"]);
    repo.write_file("partial.txt", "staged\nextra unstaged\n");

    repo.write_file("untracked.txt", "untracked\n");

    assert_failure(repo.stagelint(&["--stash", "untracked"]));

    assert_eq!(repo.read_file("partial.txt"), "staged\nextra unstaged\n");
    assert_eq!(repo.git(&["show", ":partial.txt"]), "staged\n");
    assert_eq!(repo.read_file("untracked.txt"), "untracked\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        !stash_list.contains("stagelint"),
        "stash ref should be dropped after failure: {stash_list}"
    );
}

/// `--stash untracked` hides a renamed-on-disk file (untracked destination), then restores it.
#[test]
fn stash_untracked_hides_rename_destination() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'ls *.txt > manifest.txt'", "pass_filenames": false}
    }));

    repo.write_file("old.txt", "original content\n");
    repo.write_file("trigger.txt", "trigger\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add files"]);

    repo.write_file("trigger.txt", "updated trigger\n");
    repo.git(&["add", "trigger.txt"]);

    repo.rename_file("old.txt", "new.txt");

    assert_success(repo.stagelint(&["--stash", "untracked"]));

    let manifest = repo.read_file("manifest.txt");
    assert!(
        !manifest.contains("new.txt"),
        "rename destination should be hidden during the run, manifest: {manifest}"
    );

    assert_eq!(repo.read_file("new.txt"), "original content\n");
    assert!(
        !repo.root.join("old.txt").exists(),
        "old.txt should remain absent after restore"
    );
}

/// `--stash untracked` round-trips a staged file whose parent directory became an untracked file.
#[test]
fn stash_untracked_restores_directory_file_conflict() {
    let repo = TestRepo::new(&json!({"*.txt": "true"}));

    repo.write_file("trigger.txt", "staged\n");
    repo.git(&["add", "trigger.txt"]);

    repo.write_file("build/out.txt", "staged nested\n");
    repo.git(&["add", "build/out.txt"]);
    fs::remove_dir_all(repo.root.join("build")).expect("remove build dir");
    repo.write_file("build", "now a file\n");

    assert_success(repo.stagelint(&["--stash", "untracked"]));

    assert_eq!(repo.read_file("build"), "now a file\n");
    assert_eq!(repo.git(&["show", ":build/out.txt"]), "staged nested\n");
}

/// `--stash untracked` skips a nested git repository instead of aborting.
#[test]
fn stash_untracked_skips_nested_repo() {
    let repo = TestRepo::new(&json!({"*.txt": "true"}));

    repo.write_file("trigger.txt", "staged\n");
    repo.git(&["add", "trigger.txt"]);

    repo.git(&["init", "nested"]);

    assert_success(repo.stagelint(&["--stash", "untracked"]));

    assert!(repo.root.join("nested/.git").exists());
}

/// A stash-creation failure must leave the working tree untouched.
#[test]
fn stash_failure_leaves_worktree_untouched() {
    let repo = TestRepo::new(&json!({"*.txt": "true"}));

    repo.write_file("dirty.txt", "v0\n");
    repo.git(&["add", "dirty.txt"]);
    repo.git(&["commit", "-m", "add dirty"]);
    repo.write_file("dirty.txt", "v0\nunstaged edits\n");

    repo.write_file("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);

    // No committer identity: create_stash_commit fails before a stash exists.
    repo.git(&["config", "--unset", "user.email"]);
    repo.git(&["config", "--unset", "user.name"]);

    assert_failure(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(
        repo.read_file("dirty.txt"),
        "v0\nunstaged edits\n",
        "unstaged edits must survive a failed stash"
    );
}

/// Stash handles files in nested subdirectories correctly.
#[test]
fn stash_handles_subdirectories() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in \"$@\"; do echo MODIFIED > \"$f\"; done' _"
    }));

    repo.write_file("src/lib/foo.txt", "original\n");
    repo.git(&["add", "src/lib/foo.txt"]);
    repo.git(&["commit", "-m", "add nested file"]);
    repo.write_file("src/lib/foo.txt", "dirty tracked\n");

    repo.write_file("src/bar.txt", "staged\n");
    repo.git(&["add", "src/bar.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(repo.read_file("src/lib/foo.txt"), "dirty tracked\n");
}

/// Stash correctly handles a mix of staged deletions and dirty tracked files.
#[test]
fn stash_with_staged_deletion() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("keep.txt", "original\n");
    repo.write_file("delete-me.txt", "to be deleted\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add files"]);

    repo.write_file("keep.txt", "dirty\n");
    repo.git(&["rm", "delete-me.txt"]);

    repo.write_file("new.txt", "hello\n");
    repo.git(&["add", "new.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(repo.read_file("keep.txt"), "dirty\n");
    assert!(!repo.root.join("delete-me.txt").exists());
    assert_eq!(repo.git(&["show", ":new.txt"]), "HELLO\n");
}

/// Multiple pre-existing user stashes are preserved in order.
#[test]
fn preserves_multiple_user_stashes() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "original\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "add file"]);

    repo.write_file("file.txt", "stash one\n");
    repo.git(&["stash", "push", "-m", "first stash"]);

    repo.write_file("file.txt", "stash two\n");
    repo.git(&["stash", "push", "-m", "second stash"]);

    repo.write_file("other.txt", "hello\n");
    repo.git(&["add", "other.txt"]);

    assert_success(repo.stagelint(&[]));

    let list = repo.git(&["stash", "list"]);
    assert!(
        list.contains("second stash"),
        "second stash should be preserved, got: {list}"
    );
    assert!(
        list.contains("first stash"),
        "first stash should be preserved, got: {list}"
    );

    repo.git(&["commit", "-m", "formatted"]);

    repo.git(&["stash", "pop"]);
    assert_eq!(repo.read_file("file.txt"), "stash two\n");

    repo.git(&["checkout", "file.txt"]);

    repo.git(&["stash", "pop"]);
    assert_eq!(repo.read_file("file.txt"), "stash one\n");
}

/// A non-UTF-8 committer name in the reflog must not break dropping the stash; git reads reflogs
/// as raw bytes.
#[test]
fn drop_stash_with_non_utf8_committer() {
    let repo = TestRepo::new(&json!({"*.txt": "true"}));

    // Latin-1 e-acute: invalid UTF-8, legal in git config and reflog lines.
    let mut config = fs::read(repo.root.join(".git/config")).expect("read config");
    config.extend_from_slice(b"[user]\n\tname = Ren\xe9\n\temail = rene@example.com\n");
    fs::write(repo.root.join(".git/config"), config).expect("write config");

    // Partially staged: forces a stash whose reflog line carries the committer.
    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "hello\nextra\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&["stash", "list"]),
        "",
        "backup stash must be dropped"
    );
}

/// Stagelint's stash entry is removed by OID even after a mid-run user stash displaces it.
#[test]
fn stash_ref_removed_when_not_at_top() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    // notes.md: dirty but not matched by *.txt, so stagelint ignores it; we stash it mid-run.
    repo.write_file("notes.md", "original\n");
    repo.git(&["add", "notes.md"]);
    repo.git(&["commit", "-m", "add notes"]);
    repo.write_file("notes.md", "modified\n");

    // file.txt: partially staged - triggers stagelint's stash.
    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "hello\nextra\n");

    let child = repo.stagelint(&[]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));

    // stagelint's stash is now at stash@{0}; push a user stash to displace it to stash@{1}.
    repo.git(&["stash", "push", "-m", "user stash", "--", "notes.md"]);

    repo.release_sentinel(1);
    assert_success(child);

    let list = repo.git(&["stash", "list"]);
    assert!(
        list.contains("user stash"),
        "user stash should be preserved: {list}"
    );
    assert!(
        !list.contains("stagelint"),
        "stagelint stash should be dropped: {list}"
    );

    // Verify refs/stash was updated to point at the user stash, not deleted or left stale.
    repo.git(&["stash", "pop"]);
    assert_eq!(repo.read_file("notes.md"), "modified\n");
}

/// Dropping the stash ref mid-run does not prevent a successful restore.
#[test]
fn survives_mid_run_stash_ref_drop() {
    let repo = TestRepo::new(&json!({
        "*.txt": [UPPERCASE, {"command": "git stash drop", "pass_filenames": false}]
    }));

    repo.write_file("hello.txt", "hello\nWORLD\n");
    repo.git(&["add", "hello.txt"]);
    repo.write_file("hello.txt", "hello\nWORLD\ngoodbye\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\nWORLD\n");
    assert_eq!(repo.read_file("hello.txt"), "HELLO\nWORLD\ngoodbye\n");
}

/// Closed output must not abort cleanup: the stash is dropped and the unstaged change survives.
#[cfg(unix)]
#[test]
fn closed_output_does_not_leak_stash() {
    let repo = TestRepo::new(&json!({"*.txt": "echo OUTPUT"}));

    repo.write_file("file.txt", "v1\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "v2\n");

    let mut child = repo.stagelint(&[]);
    // Both, so a broken pipe is hit whichever stream the output goes to.
    drop((child.stdout.take(), child.stderr.take()));
    assert_success(child);

    assert_eq!(repo.read_file("file.txt"), "v2\n");
    assert!(repo.git(&["stash", "list"]).is_empty());
}

/// Exec bit tracks a stash round-trip: set while hidden, cleared on restore.
#[cfg(unix)]
#[test]
fn stash_round_trip_tracks_executable_bit() {
    use std::os::unix::fs::PermissionsExt;

    let repo = TestRepo::new(&json!({"*.sh": sentinel(1)}));

    repo.write_file("foo.sh", "echo hi\n");
    let foo = repo.root.join("foo.sh");
    fs::set_permissions(&foo, fs::Permissions::from_mode(0o755)).unwrap();
    repo.git(&["add", "foo.sh"]);
    repo.git(&["commit", "-m", "add executable foo.sh"]);

    repo.write_file("foo.sh", "echo bye\n");
    repo.git(&["add", "foo.sh"]);
    fs::set_permissions(&foo, fs::Permissions::from_mode(0o644)).unwrap();

    let child = repo.stagelint(&[]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));

    let hidden = fs::metadata(&foo).unwrap().permissions().mode();
    assert_ne!(
        hidden & 0o111,
        0,
        "exec bit should be set while hidden, got {hidden:o}"
    );

    repo.release_sentinel(1);
    assert_success(child);

    let restored = fs::metadata(&foo).unwrap().permissions().mode();
    assert_eq!(
        restored & 0o111,
        0,
        "exec bit should be cleared after restore, got {restored:o}"
    );
    assert_eq!(repo.read_file("foo.sh"), "echo bye\n");
}

// Partial staging

/// Partially staged file: linter changes reach index and working tree, unstaged lines preserved.
#[test]
fn partial_stage_linter_modifies() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "hello\nWORLD\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "hello\nWORLD\nextra line\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "HELLO\nWORLD\n");

    assert_eq!(repo.read_file("file.txt"), "HELLO\nWORLD\nextra line\n");
}

/// Partially staged file: linter modifies then fails; state is still fully restored.
#[test]
fn partial_stage_modify_then_fail_restores() {
    let repo = TestRepo::new(&json!({"*.txt": [UPPERCASE, "false"]}));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "staged\nworking tree extra\n");

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.read_file("file.txt"), "staged\nworking tree extra\n");
    assert_eq!(repo.git(&["show", ":file.txt"]), "staged\n");
}

/// When the linter output matches the working tree, the post-run diff is empty.
#[test]
fn partial_stage_linter_matches_workdir() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "HELLO\n");

    assert_success(repo.stagelint(&[]));

    let diff = repo.git(&["diff", "file.txt"]);
    assert!(
        diff.is_empty(),
        "git diff should be empty after linter applied same changes, got: {diff}"
    );
    assert_eq!(repo.git(&["show", ":file.txt"]), "HELLO\n");
}

/// `pass_filenames: false` with partial staging: correct staged content, working tree preserved.
#[test]
fn partial_stage_pass_filenames_false() {
    let repo = TestRepo::new(&json!({
        "*.txt": {
            "command": "sh -c 'for f in *.txt; do tr a-z A-Z < \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done'",
            "pass_filenames": false
        }
    }));

    repo.write_file("hello.txt", "hello world\n");
    repo.git(&["add", "hello.txt"]);
    repo.write_file("hello.txt", "hello world modified\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO WORLD\n");
}

/// A hidden file whose worktree content equals HEAD is still restored after the run.
#[test]
fn partial_stage_restores_head_identical_content() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "v1\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "add file"]);
    repo.write_file("file.txt", "v2\n");
    repo.git(&["add", "file.txt"]);
    // Worktree back to the committed content: still dirty against the index.
    repo.write_file("file.txt", "v1\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "V2\n");
    assert_eq!(
        repo.read_file("file.txt"),
        "v1\n",
        "worktree content matching HEAD must still be restored"
    );
}

/// When the linter and working tree edit the same line, the working tree wins.
#[test]
fn partial_stage_conflict_workdir_wins() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line1", "REPLACED")}));

    repo.write_file("file.txt", "line1\nline2\n");
    repo.git(&["add", "file.txt"]);
    // Working tree also changes line1 - conflicts with the linter's replacement.
    repo.write_file("file.txt", "modified_line1\nline2\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "REPLACED\nline2\n");
    assert_eq!(repo.read_file("file.txt"), "modified_line1\nline2\n");
}

/// On conflict, the whole file is left untouched: no partial changes are applied.
#[test]
fn partial_stage_conflict_skips_whole_file() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in \"$@\"; do sed -e s/line1/ONE/ -e s/line9/NINE/ \"$f\" > \"$f.t\" && mv \"$f.t\" \"$f\"; done' _"
    }));

    let base = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\n";
    repo.write_file("file.txt", base);
    repo.git(&["add", "file.txt"]);
    // The working tree edit conflicts with the line1 hunk but not the line9 hunk.
    let working = base.replace("line1", "edited_line1");
    repo.write_file("file.txt", &working);

    let output = assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.read_file("file.txt"),
        working,
        "a conflicted file must not receive partial changes"
    );
    assert!(repo.git(&["show", ":file.txt"]).contains("NINE"));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("conflict"),
        "the skipped file should be warned about"
    );
}

/// A `merge=binary` gitattribute prevents text-merging changes into the working tree.
#[test]
fn merge_binary_attribute_skips_worktree_merge() {
    let repo = TestRepo::new(&json!({"*.bin": replace("v1", "v2")}));

    repo.write_file(".gitattributes", "*.bin merge=binary\n");
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "attrs"]);

    repo.write_file("data.bin", "v1\npayload\n");
    repo.git(&["add", "data.bin"]);
    // Non-conflicting worktree edit: a text merge would apply cleanly.
    repo.write_file("data.bin", "v1\npayload\nextra\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":data.bin"]), "v2\npayload\n");
    assert_eq!(
        repo.read_file("data.bin"),
        "v1\npayload\nextra\n",
        "merge=binary files must not be text-merged into the worktree"
    );
}

/// Binary content is never text-merged, even without a gitattribute.
#[test]
fn binary_content_skips_worktree_merge() {
    let repo = TestRepo::new(&json!({"*.dat": replace("v1", "v2")}));

    let base = b"BIN\x00\nv1\nl3\n";
    fs::write(repo.root.join("data.dat"), base).expect("write binary");
    repo.git(&["add", "data.dat"]);
    let working = b"BIN\x00\nv1\nl3x\n";
    fs::write(repo.root.join("data.dat"), working).expect("write binary");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        fs::read(repo.root.join("data.dat")).expect("read binary"),
        working,
        "binary files must not be text-merged into the worktree"
    );
    let indexed = repo.git_cmd(&["show", ":data.dat"]);
    assert!(indexed.status.success());
    assert!(
        indexed.stdout.windows(2).any(|w| w == b"v2"),
        "the staged content should still be formatted"
    );
}

/// An `eol=crlf` attribute: the merge result is written in worktree form, not git form.
#[test]
fn partial_stage_merge_respects_eol_attribute() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line2", "FORMATTED")}));

    repo.write_file(".gitattributes", "*.txt text eol=crlf\n");
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "attrs"]);

    // The index stores LF (clean filter); the worktree keeps CRLF.
    repo.write_file("file.txt", "line1\r\nline2\r\nline3\r\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "line1\r\nline2\r\nline3\r\nextra\r\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "line1\nFORMATTED\nline3\n"
    );
    assert_eq!(
        repo.read_file("file.txt"),
        "line1\r\nFORMATTED\r\nline3\r\nextra\r\n",
        "the merge-back must write CRLF"
    );
}

/// An external smudge filter converts the merge result to worktree form.
#[test]
fn partial_stage_merge_applies_smudge_filter() {
    let repo = TestRepo::new(&json!({"*.txt": replace("LINE2", "FORMATTED")}));

    repo.write_file(".gitattributes", "*.txt filter=case\n");
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["config", "filter.case.clean", "tr A-Z a-z"]);
    repo.git(&["config", "filter.case.smudge", "tr a-z A-Z"]);
    repo.git(&["commit", "-m", "attrs"]);

    // Git form is lowercase, worktree form uppercase; the linter sees worktree form.
    repo.write_file("file.txt", "line1\nline2\nline3\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "LINE1\nLINE2\nLINE3\nEXTRA\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "line1\nformatted\nline3\n",
        "staged content must be clean-filtered to git form"
    );
    assert_eq!(
        repo.read_file("file.txt"),
        "LINE1\nFORMATTED\nLINE3\nEXTRA\n",
        "the merge-back must pass through the smudge filter"
    );
}

/// The linter's output is clean-filtered when staged, exactly as `git add` would.
#[test]
fn linter_output_is_clean_filtered() {
    let repo = TestRepo::new(&json!({"*.txt": replace("hello", "HELLO")}));

    // Worktree form carries a "W:" line prefix; git form has it stripped.
    repo.write_file(".gitattributes", "*.txt filter=wrap\n");
    repo.git(&["config", "filter.wrap.clean", "sed s/^W://"]);
    repo.git(&["config", "filter.wrap.smudge", "sed s/^/W:/"]);
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "attrs"]);

    repo.write_file("file.txt", "W:hello\n");
    repo.git(&["add", "file.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "HELLO\n",
        "staged content must be in git form"
    );
    assert_eq!(repo.read_file("file.txt"), "W:HELLO\n");
}

/// During the run, hidden files hold smudged (worktree-form) staged content.
#[test]
fn run_sees_worktree_form_content() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'cat file.txt > seen.txt'", "pass_filenames": false}
    }));

    repo.write_file(".gitattributes", "*.txt filter=wrap\n");
    repo.git(&["config", "filter.wrap.clean", "sed s/^W://"]);
    repo.git(&["config", "filter.wrap.smudge", "sed s/^/W:/"]);
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "attrs"]);

    repo.write_file("file.txt", "W:one\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "W:one\nW:two\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.read_file("seen.txt"),
        "W:one\n",
        "the linter must see worktree-form content"
    );
    assert_eq!(repo.read_file("file.txt"), "W:one\nW:two\n");
}

/// The stash stores git-form blobs so `git stash pop` recovery smudges them correctly.
#[test]
fn crash_stash_holds_git_form_content() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file(".gitattributes", "*.txt filter=wrap\n");
    repo.git(&["config", "filter.wrap.clean", "sed s/^W://"]);
    repo.git(&["config", "filter.wrap.smudge", "sed s/^/W:/"]);
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "attrs"]);

    repo.write_file("file.txt", "W:base\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "add file"]);
    repo.write_file("file.txt", "W:dirty\n");

    repo.write_file("staged.txt", "s\n");
    repo.git(&["add", "staged.txt"]);

    let mut child = repo.stagelint(&["--stash", "tracked"]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    child.kill().unwrap();
    child.wait().unwrap();

    repo.git(&["reset", "--hard", "HEAD"]);
    repo.git(&["stash", "pop"]);

    assert_eq!(
        repo.read_file("file.txt"),
        "W:dirty\n",
        "recovery must smudge the stashed content exactly once"
    );
}

/// The side-effect check compares in git form: a stat-stale filtered file is not rewritten.
#[test]
fn revert_keeps_worktree_form() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'touch keep.txt'", "pass_filenames": false}
    }));

    repo.write_file(".gitattributes", "*.txt filter=wrap\n");
    repo.git(&["config", "filter.wrap.clean", "sed s/^W://"]);
    repo.git(&["config", "filter.wrap.smudge", "sed s/^/W:/"]);
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["commit", "-m", "attrs"]);

    repo.write_file("keep.txt", "W:keep\n");
    repo.git(&["add", "keep.txt"]);
    repo.git(&["commit", "-m", "add keep"]);

    repo.write_file("staged.txt", "s\n");
    repo.git(&["add", "staged.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(
        repo.read_file("keep.txt"),
        "W:keep\n",
        "a clean filtered file must not be rewritten to git form"
    );
}

/// A failing merge driver counts as a conflict: warn and leave the worktree untouched.
#[test]
fn merge_driver_failure_warns_and_skips() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line2", "FORMATTED")}));

    repo.write_file(".gitattributes", "*.txt merge=broken\n");
    repo.git(&["add", ".gitattributes"]);
    repo.git(&["config", "merge.broken.driver", "false"]);
    repo.git(&["commit", "-m", "attrs"]);

    repo.write_file("file.txt", "line1\nline2\nline3\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "line1\nline2\nline3\nextra\n");

    let output = assert_success(repo.stagelint(&[]));

    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[WARN] Changes staged but not applied"),
        "driver failure should be warned about, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(repo.read_file("file.txt"), "line1\nline2\nline3\nextra\n");
    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "line1\nFORMATTED\nline3\n"
    );
}

/// `--quiet` suppresses merge warnings.
#[test]
fn quiet_suppresses_merge_warnings() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line1", "REPLACED")}));

    repo.write_file("file.txt", "line1\nline2\n");
    repo.git(&["add", "file.txt"]);
    // Conflicts with the linter's replacement: warns without --quiet.
    repo.write_file("file.txt", "modified_line1\nline2\n");

    let output = assert_success(repo.stagelint(&["--quiet"]));

    assert!(
        output.stderr.is_empty(),
        "--quiet must suppress warnings, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// Mid-merge

/// Runs correctly mid-merge: conflict resolved and staged, `MERGE_HEAD` still set.
#[test]
fn merge_conflict_resolution_formatted() {
    let repo = TestRepo::new(&json!({"*.txt": replace("hello", "HELLO")}));

    repo.write_file("file.txt", "initial\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "add file"]);

    repo.git(&["checkout", "-b", "branch-a"]);
    repo.write_file("file.txt", "content from a\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "branch-a"]);

    repo.git(&["checkout", "main"]);
    repo.git(&["checkout", "-b", "branch-b"]);
    repo.write_file("file.txt", "content from b\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "branch-b"]);

    let merge_output = repo.git_cmd(&["merge", "branch-a"]);
    assert!(
        !merge_output.status.success(),
        "merge should produce conflict"
    );

    repo.write_file("file.txt", "hello resolved\n");
    repo.git(&["add", "file.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "HELLO resolved\n");
    repo.git(&["rev-parse", "--verify", "MERGE_HEAD"]);
}

/// On linter failure mid-merge, the index is restored to the resolved state.
#[test]
fn merge_conflict_linter_failure_restores() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("file.txt", "initial\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "add file"]);

    repo.git(&["checkout", "-b", "branch-a"]);
    repo.write_file("file.txt", "content-a\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "branch-a"]);

    repo.git(&["checkout", "main"]);
    repo.git(&["checkout", "-b", "branch-b"]);
    repo.write_file("file.txt", "content-b\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "branch-b"]);

    let merge_output = repo.git_cmd(&["merge", "branch-a"]);
    assert!(
        !merge_output.status.success(),
        "merge should produce conflict"
    );

    repo.write_file("file.txt", "resolved content\n");
    repo.git(&["add", "file.txt"]);

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "resolved content\n");
    repo.git(&["rev-parse", "--verify", "MERGE_HEAD"]);
}

// Renames and deletions

/// Unstaged rename (no `git mv`): the old index entry is formatted, the rename preserved.
#[test]
fn unstaged_rename_preserved() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("old.txt", "original\n");
    repo.git(&["add", "old.txt"]);
    repo.git(&["commit", "-m", "add old.txt"]);

    repo.write_file("old.txt", "hello world\n");
    repo.git(&["add", "old.txt"]);

    repo.rename_file("old.txt", "new.txt");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":old.txt"]), "HELLO WORLD\n");

    assert!(
        !repo.root.join("old.txt").exists(),
        "old.txt should be absent after restore"
    );
    assert!(
        repo.root.join("new.txt").exists(),
        "new.txt should still exist"
    );
    assert!(
        repo.git(&["stash", "list"]).is_empty(),
        "stash should be cleaned up"
    );
}

/// Unstaged rename with failing linter: the rename state is preserved even on failure.
#[test]
fn unstaged_rename_failure_preserved() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("old.txt", "original\n");
    repo.git(&["add", "old.txt"]);
    repo.git(&["commit", "-m", "add old.txt"]);

    repo.write_file("old.txt", "hello world\n");
    repo.git(&["add", "old.txt"]);

    repo.rename_file("old.txt", "new.txt");

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":old.txt"]), "hello world\n");
    assert!(
        !repo.root.join("old.txt").exists(),
        "old.txt should remain absent after failure restore"
    );
    assert!(
        repo.root.join("new.txt").exists(),
        "new.txt should still exist after failure"
    );
    assert!(
        repo.git(&["stash", "list"]).is_empty(),
        "stash should be cleaned up after failure"
    );
}

/// `git mv` rename: the new path is formatted and unstaged edits merge correctly.
#[test]
fn git_mv_formats_new_path() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line2", "FORMATTED")}));

    repo.write_file("old.txt", "line1\nline2\nline3\n");
    repo.git(&["add", "old.txt"]);
    repo.git(&["commit", "-m", "add old.txt"]);

    repo.git(&["mv", "old.txt", "new.txt"]);
    repo.write_file("new.txt", "line1\nline2\nline3\nextra unstaged\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":new.txt"]), "line1\nFORMATTED\nline3\n");
    assert_eq!(
        repo.read_file("new.txt"),
        "line1\nFORMATTED\nline3\nextra unstaged\n"
    );

    let ls_old = repo.git(&["ls-files", "old.txt"]);
    assert!(ls_old.is_empty(), "old.txt should not be in index");
    assert!(
        !repo.root.join("old.txt").exists(),
        "old.txt should not exist on disk"
    );
}

/// Staged deletions are not passed to the linter; the file remains deleted after the run.
#[test]
fn staged_delete_skipped() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("keep.txt", "hello\n");
    repo.write_file("delete-me.txt", "content\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "add files"]);

    repo.git(&["rm", "delete-me.txt"]);
    repo.write_file("keep.txt", "hello updated\n");
    repo.git(&["add", "keep.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":keep.txt"]), "HELLO UPDATED\n");

    assert!(!repo.root.join("delete-me.txt").exists());
    let ls = repo.git(&["ls-files", "delete-me.txt"]);
    assert!(ls.is_empty(), "delete-me.txt should not be in index");
}

/// A staged-deleted file is not resurrected when the linter fails.
#[test]
fn staged_delete_not_resurrected_on_failure() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));
    repo.write_file("to-delete.txt", "delete me\n");
    repo.git(&["add", "to-delete.txt"]);
    repo.git(&["commit", "-m", "add file to delete"]);
    repo.git(&["rm", "to-delete.txt"]);
    repo.write_file("other.txt", "content\n");
    repo.git(&["add", "other.txt"]);
    assert_failure(repo.stagelint(&[]));
    assert!(
        !repo.root.join("to-delete.txt").exists(),
        "to-delete.txt should remain absent after linter failure"
    );
}

/// A staged file deleted from the worktree: its staged content is formatted, the deletion stays.
#[test]
fn staged_file_deleted_from_worktree_formats_staged_version() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("gone.txt", "content\n");
    repo.git(&["add", "gone.txt"]);
    fs::remove_file(repo.root.join("gone.txt")).expect("delete gone");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":gone.txt"]), "CONTENT\n");
    assert!(
        !repo.root.join("gone.txt").exists(),
        "staged deletion should remain absent from the worktree"
    );
}

// Runner

/// Commands in a pipeline run sequentially; each one sees the file changes made by the previous.
#[test]
fn multiple_commands_sequential() {
    let repo = TestRepo::new(&json!({
        "*.txt": [replace("1", "2"), replace("2", "3")]
    }));

    repo.write_file("file.txt", "1\n");
    repo.git(&["add", "file.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "3\n");
}

/// Bare command names must find npm's `.cmd` shims on PATH - npm ships no `.exe`.
#[cfg(windows)]
#[test]
fn windows_resolves_cmd_shims() {
    let repo = TestRepo::new(&json!({"*.txt": "tool"}));

    repo.write_file(
        ".git/bin/tool.cmd",
        "@echo off\r\necho ran> .git\\shim-ran\r\n",
    );

    repo.write_file("hello.txt", "content\n");
    repo.git(&["add", "hello.txt"]);

    let mut path_var = repo.root.join(".git/bin").into_os_string();
    path_var.push(";");
    path_var.push(std::env::var_os("PATH").unwrap_or_default());
    let child = repo
        .stagelint_cmd()
        .env("PATH", path_var)
        .spawn()
        .expect("spawn stagelint");
    assert_success(child);

    assert!(
        repo.root.join(".git/shim-ran").exists(),
        "the .cmd shim must have run"
    );
}

/// With `pass_filenames: false`, no filenames are appended to the command.
#[test]
fn pass_filenames_false() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'echo $# > args-count.txt'", "pass_filenames": false}
    }));

    repo.write_file("hello.txt", "content\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    let count = repo.read_file("args-count.txt");
    assert_eq!(
        count.trim(),
        "0",
        "no filenames should be passed, got: {count}"
    );
}

/// Commands must not inherit stagelint's stdin: a command reading it would hang forever.
#[test]
fn command_stdin_is_closed() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'cat'", "pass_filenames": false}
    }));

    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);

    // The pipe stays open for the whole run; an inheriting `cat` would block on it.
    let mut cmd = repo.stagelint_cmd();
    let child = cmd.stdin(Stdio::piped()).spawn().expect("spawn stagelint");

    assert_success(child);
}

/// Without `--continue-on-error`, the failure stops the rest of its pipeline and the other tasks.
#[test]
fn default_stops_on_first_error() {
    // The sleep outlasts the failure, so the second command is only reached after the cancel.
    let repo = TestRepo::new(&json!({
        "*.txt": ["false", "sh -c 'echo ran > marker.txt'"],
        "*.md": ["sleep 1", "sh -c 'echo ran > sibling.txt'"],
    }));

    repo.write_file("file.txt", "content\n");
    repo.write_file("file.md", "content\n");
    repo.git(&["add", "file.txt", "file.md"]);

    assert_failure(repo.stagelint(&[]));
    assert!(
        !repo.root.join("marker.txt").exists(),
        "marker file should not exist without --continue-on-error"
    );
    assert!(
        !repo.root.join("sibling.txt").exists(),
        "the other task must be cancelled too"
    );
    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "content\n",
        "index should be unchanged when pipeline aborts"
    );
}

/// `--continue-on-error`: every command and task runs despite a failure; exit code still non-zero.
#[test]
fn continue_on_error_runs_all_on_failure() {
    let repo = TestRepo::new(&json!({
        "*.txt": ["false", "sh -c 'echo ran > marker.txt'"],
        "*.md": ["sleep 1", "sh -c 'echo ran > sibling.txt'"],
    }));

    repo.write_file("file.txt", "content\n");
    repo.write_file("file.md", "content\n");
    repo.git(&["add", "file.txt", "file.md"]);

    assert_failure(repo.stagelint(&["--continue-on-error"]));
    assert!(
        repo.root.join("marker.txt").exists(),
        "marker file should exist with --continue-on-error (second command ran)"
    );
    assert!(
        repo.root.join("sibling.txt").exists(),
        "the other task must run to completion too"
    );
}

/// `--continue-on-error`: a command that fails to spawn does not stop its pipeline.
#[test]
fn continue_on_error_runs_pipeline_after_spawn_failure() {
    let repo = TestRepo::new(&json!({
        "*.txt": ["cmd-that-does-not-exist-stagelint-test", "sh -c 'echo ran > marker.txt'"]
    }));

    repo.write_file("file.txt", "content\n");
    repo.git(&["add", "file.txt"]);

    let output = assert_failure(repo.stagelint(&["--continue-on-error"]));

    assert!(
        repo.root.join("marker.txt").exists(),
        "the pipeline should continue past a spawn failure"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to run"),
        "the spawn failure should be reported"
    );
}

/// `--concurrent 2` caps running tasks at two: the third starts only when a slot frees.
#[test]
fn concurrent_cap_limits_running_tasks() {
    // serde_json sorts the keys, so *.md and *.rs run first and *.txt waits for a slot.
    let repo = TestRepo::new(&json!({
        "*.md": sentinel(1),
        "*.rs": sentinel(2),
        "*.txt": sentinel(3),
    }));

    repo.write_file("file.md", "hello\n");
    repo.git(&["add", "file.md"]);
    repo.write_file("file.rs", "hello\n");
    repo.git(&["add", "file.rs"]);
    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);

    let child = repo.stagelint(&["--concurrent", "2"]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    assert!(repo.wait_sentinel(2, Duration::from_secs(10)));

    // Both slots occupied - task 3 must not start. Wait briefly to catch any premature dispatch.
    assert!(
        !repo.wait_sentinel(3, Duration::from_millis(200)),
        "task 3 should not start while both slots are full"
    );

    repo.release_sentinel(1);
    assert!(repo.wait_sentinel(3, Duration::from_secs(10)));

    repo.release_sentinel(2);
    repo.release_sentinel(3);
    assert_success(child);
}

/// With `--concurrent`, a task failure cancels other running tasks.
#[test]
fn concurrent_failure_cancels_running_tasks() {
    let repo = TestRepo::new(&json!({
        "*.md": [sentinel(1), "false"],
        "*.rs": sentinel(2),
    }));

    repo.write_file("file.md", "hello\n");
    repo.git(&["add", "file.md"]);
    repo.write_file("file.rs", "hello\n");
    repo.git(&["add", "file.rs"]);

    let child = repo.stagelint(&["--concurrent", "2"]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    assert!(repo.wait_sentinel(2, Duration::from_secs(10)));

    // Release task 1: sentinel exits 0, then "false" runs and fails.
    repo.release_sentinel(1);

    // Task 2 would block forever if not killed; stagelint exiting proves it was cancelled.
    let output = assert_failure(child);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[CANCELLED] .stagelint.json > *.rs > "),
        "sibling not cancelled: {stderr}"
    );
    assert!(
        stderr.contains("[FAILED] .stagelint.json > *.md > false\n"),
        "failure not reported: {stderr}"
    );
}

/// A task already exiting when the cancel reaches it keeps its own status, not `Cancelled`.
#[cfg(unix)]
#[test]
fn exiting_task_is_not_reported_as_cancelled() {
    // Both tasks block until released together, so their failures are in flight at once.
    let repo = TestRepo::new(&json!({
        "*.md": "sh -c 'echo MD-ERROR; read x < .git/gate-1; exit 1'",
        "*.rs": "sh -c 'echo RS-ERROR; read x < .git/gate-2; exit 1'",
    }));

    repo.write_file("file.md", "hello\n");
    repo.git(&["add", "file.md"]);
    repo.write_file("file.rs", "hello\n");
    repo.git(&["add", "file.rs"]);

    let gates = [repo.root.join(".git/gate-1"), repo.root.join(".git/gate-2")];
    let status = Command::new("mkfifo")
        .args(&gates)
        .status()
        .expect("mkfifo");
    assert!(status.success(), "mkfifo failed");

    let child = repo.stagelint(&["--concurrent", "2"]);

    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let writers: std::io::Result<Vec<_>> = gates.iter().map(fs::File::create).collect();
        tx.send(writers).ok();
    });
    let writers = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("tasks never opened their gates")
        .expect("open gates");
    drop(writers); // EOF releases both tasks

    let output = assert_failure(child);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for block in [
        "\nMD-ERROR\nexit status: 1\n",
        "\nRS-ERROR\nexit status: 1\n",
    ] {
        assert!(stderr.contains(block), "missing {block:?} in: {stderr}");
    }
}

/// Passing commands stay quiet unless `--verbose` asks for their output.
#[test]
fn verbose_shows_passing_output() {
    let repo = TestRepo::new(&json!({ "*.md": "sh -c 'echo LINT-OK'" }));
    repo.write_file("file.md", "hello\n");
    repo.git(&["add", "file.md"]);

    let output = assert_success(repo.stagelint(&[]));
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("LINT-OK\n"),
        "passing output should be hidden by default"
    );

    let output = assert_success(repo.stagelint(&["--verbose"]));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("LINT-OK\n"),
        "--verbose should show passing output"
    );
}

// Sources

/// A revspec that is not a diff range is rejected.
#[test]
fn diff_rejects_non_range_spec() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    let output = assert_failure(repo.stagelint(&["--diff", "^HEAD"]));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not a diff range"),
        "got: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let output = assert_failure(repo.stagelint(&["--diff", "no-such-ref"]));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("failed to resolve revision"),
        "got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--diff a..b` lints the files changed between the two revisions, not the staged set.
#[test]
fn diff_range_lints_changed_files() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("changed.txt", "changed\n");
    repo.git(&["add", "changed.txt"]);
    repo.git(&["commit", "-m", "change"]);
    repo.write_file("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);

    assert_success(repo.stagelint(&["--diff", "HEAD~1..HEAD"]));

    assert_eq!(repo.read_file("changed.txt"), "CHANGED\n");
    assert_eq!(repo.read_file("staged.txt"), "staged\n");
}

/// `--diff a...b` diffs from the merge base: a file the other branch changed since the fork is
/// out of scope, where `a..b` would include it.
#[test]
fn diff_merge_base_excludes_other_branch() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("shared.txt", "base\n");
    repo.git(&["add", "shared.txt"]);
    repo.git(&["commit", "-m", "base"]);
    repo.git(&["checkout", "-qb", "feature"]);
    repo.write_file("feature.txt", "feature\n");
    repo.git(&["add", "feature.txt"]);
    repo.git(&["commit", "-m", "feature"]);
    repo.git(&["checkout", "-q", "-"]);
    repo.write_file("shared.txt", "main\n");
    repo.git(&["add", "shared.txt"]);
    repo.git(&["commit", "-m", "main"]);
    repo.git(&["checkout", "-q", "feature"]);

    assert_success(repo.stagelint(&["--diff", "main...HEAD"]));

    assert_eq!(repo.read_file("feature.txt"), "FEATURE\n");
    assert_eq!(
        repo.read_file("shared.txt"),
        "base\n",
        "changed on main since the fork: in main..HEAD, not in main...HEAD"
    );
}

/// A single revision under `--diff` compares against HEAD, as `git diff <rev>` does.
#[test]
fn diff_single_revision_compares_to_head() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "changed\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "change"]);

    assert_success(repo.stagelint(&["--diff", "HEAD~1"]));

    assert_eq!(repo.read_file("file.txt"), "CHANGED\n");
}

/// A file in the range that no longer exists on disk is not passed to commands.
#[test]
fn diff_skips_files_missing_from_disk() {
    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'printf \"%s\\n\" \"$@\" > .git/argv' _"}));

    repo.write_file("kept.txt", "kept\n");
    repo.write_file("gone.txt", "gone\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "change"]);
    fs::remove_file(repo.root.join("gone.txt")).unwrap();

    assert_success(repo.stagelint(&["--diff", "HEAD~1..HEAD"]));

    let argv = repo.read_file(".git/argv");
    assert!(argv.contains("kept.txt"), "got: {argv}");
    assert!(!argv.contains("gone.txt"), "got: {argv}");
}

/// Under `--diff` an unstaged edit to a file in the range is linted and its result staged.
#[test]
fn diff_stages_unstaged_edits_in_range() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("file.txt", "base\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "base"]);
    repo.write_file("file.txt", "committed\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "change"]);

    repo.write_file("file.txt", "committed\nunstaged\n");

    assert_success(repo.stagelint(&["--diff", "HEAD~1..HEAD"]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "COMMITTED\nUNSTAGED\n");
    assert_eq!(repo.read_file("file.txt"), "COMMITTED\nUNSTAGED\n");
}

/// Modified, untracked and partially staged files are linted; a staged file matching the index and
/// an ignored file are not. The index is left as it was.
#[test]
fn unstaged_lints_worktree_changes_without_staging() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file(".gitignore", "ignored.txt\n");
    repo.write_file("modified.txt", "modified\n");
    repo.write_file("partial.txt", "partial\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "base"]);

    repo.write_file("modified.txt", "modified again\n");
    repo.write_file("partial.txt", "partial\nstaged\n");
    repo.git(&["add", "partial.txt"]);
    repo.write_file("partial.txt", "partial\nstaged\nworking\n");
    repo.write_file("staged.txt", "staged\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("untracked.txt", "untracked\n");
    repo.write_file("ignored.txt", "ignored\n");

    assert_success(repo.stagelint(&["--unstaged"]));

    assert_eq!(repo.read_file("modified.txt"), "MODIFIED AGAIN\n");
    assert_eq!(repo.read_file("untracked.txt"), "UNTRACKED\n");
    assert_eq!(repo.read_file("partial.txt"), "PARTIAL\nSTAGED\nWORKING\n");
    assert_eq!(repo.read_file("staged.txt"), "staged\n");
    assert_eq!(repo.read_file("ignored.txt"), "ignored\n");

    assert_eq!(repo.git(&["show", ":partial.txt"]), "partial\nstaged\n");
    assert_eq!(
        repo.git(&["diff", "--cached", "--name-only"]),
        "partial.txt\nstaged.txt\n"
    );
    assert_eq!(
        repo.git(&["status", "--porcelain", "untracked.txt"]),
        "?? untracked.txt\n"
    );
}

/// A path in the range with no index entry is skipped: nothing could stage or restore it.
#[test]
fn diff_skips_path_missing_from_index() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("a.txt", "one\n");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-m", "base"]);
    repo.write_file("a.txt", "two\n");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-m", "change"]);
    repo.git(&["rm", "--cached", "a.txt"]);

    assert_success(repo.stagelint(&["--diff", "HEAD~1..HEAD"]));

    assert_eq!(repo.read_file("a.txt"), "two\n");
}

/// A file in the range replaced by a symlink is skipped, so a command cannot write through it.
#[test]
fn diff_skips_path_replaced_by_symlink() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "true"]);

    // The target does not match the glob, so only a follow-through could change it.
    repo.write_file("target.md", "secret\n");
    repo.write_file("a.txt", "one\n");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "base"]);
    repo.write_file("a.txt", "two\n");
    repo.git(&["add", "a.txt"]);
    repo.git(&["commit", "-m", "change"]);

    fs::remove_file(repo.root.join("a.txt")).expect("remove a.txt");
    symlink_file("target.md", &repo.root.join("a.txt")).expect("symlink a.txt -> target.md");

    assert_success(repo.stagelint(&["--diff", "HEAD~1..HEAD"]));

    assert_eq!(repo.read_file("target.md"), "secret\n");
}

/// A type change is judged by the worktree, not the index: a file replaced by a symlink is
/// skipped so a command cannot write through it, a symlink replaced by a file is linted.
#[test]
fn unstaged_follows_worktree_after_type_change() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "true"]);

    // The target does not match the glob, so only a follow-through could change it.
    repo.write_file("target.md", "secret\n");
    repo.write_file("now_link.txt", "original\n");
    symlink_file("target.md", &repo.root.join("now_file.txt")).expect("symlink now_file.txt");
    repo.git(&["add", "-A"]);
    repo.git(&["commit", "-m", "base"]);

    fs::remove_file(repo.root.join("now_link.txt")).expect("remove now_link.txt");
    symlink_file("target.md", &repo.root.join("now_link.txt")).expect("symlink now_link.txt");
    fs::remove_file(repo.root.join("now_file.txt")).expect("remove now_file.txt");
    repo.write_file("now_file.txt", "real\n");

    assert_success(repo.stagelint(&["--unstaged"]));

    assert_eq!(repo.read_file("target.md"), "secret\n");
    assert_eq!(repo.read_file("now_file.txt"), "REAL\n");
}

/// Only named paths are linted, absolute or relative; missing paths and directories are skipped,
/// and nothing is staged.
#[test]
fn files_lints_given_paths() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("top.txt", "top\n");
    repo.write_file("sub/nested.txt", "nested\n");
    repo.write_file("sub/other.txt", "other\n");
    repo.write_file("sub/deeper/deep.txt", "deep\n");

    let mut cmd = repo.stagelint_cmd();
    let output = cmd
        .args([
            "--files",
            repo.root.join("top.txt").to_str().expect("utf8 path"),
            "nested.txt",
            "deeper",
            "gone.txt",
        ])
        .current_dir(repo.root.join("sub"))
        .output()
        .expect("run stagelint");
    assert!(
        output.status.success(),
        "stagelint failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert_eq!(repo.read_file("top.txt"), "TOP\n");
    assert_eq!(repo.read_file("sub/nested.txt"), "NESTED\n");
    assert_eq!(repo.read_file("sub/other.txt"), "other\n");
    assert_eq!(repo.read_file("sub/deeper/deep.txt"), "deep\n");
    assert_eq!(repo.git(&["diff", "--cached", "--name-only"]), "");
}

// Config

/// Monorepo: each file uses the nearest config walking up to the root.
#[test]
fn monorepo_multiple_configs() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    let web_config = json!({"*.txt": replace("hello", "goodbye")});
    let web_config_str = serde_json::to_string(&web_config).expect("serialize");
    repo.write_file("packages/web/.stagelint.json", &web_config_str);
    repo.git(&["add", "packages/web/.stagelint.json"]);
    repo.git(&["commit", "-m", "add web config"]);

    repo.write_file("root.txt", "hello\n");
    repo.git(&["add", "root.txt"]);
    repo.write_file("packages/web/page.txt", "hello\n");
    repo.git(&["add", "packages/web/page.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":root.txt"]), "HELLO\n");

    assert_eq!(repo.git(&["show", ":packages/web/page.txt"]), "goodbye\n");
}

/// A task declared in a subdirectory config runs with that directory as its cwd.
#[test]
fn subdirectory_config_runs_in_its_own_directory() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    let web_config = json!({"*.txt": "sh -c 'echo ran > proof'"});
    let web_config_str = serde_json::to_string(&web_config).expect("serialize");
    repo.write_file("packages/web/.stagelint.json", &web_config_str);
    repo.git(&["add", "packages/web/.stagelint.json"]);
    repo.git(&["commit", "-m", "add web config"]);

    repo.write_file("packages/web/page.txt", "hello\n");
    repo.git(&["add", "packages/web/page.txt"]);

    assert_success(repo.stagelint(&[]));

    // A relative write only lands here if the task ran in packages/web.
    assert_eq!(repo.read_file("packages/web/proof"), "ran\n");
}

/// Paths reach commands absolute, so a filename that looks like a flag cannot be parsed as one.
#[test]
fn dash_prefixed_filename_passed_as_path() {
    let repo = TestRepo::new(&json!({
        "*.txt": [
            "sh -c 'for f in \"$@\"; do case \"$f\" in -*) exit 1;; esac; done' _",
            UPPERCASE
        ]
    }));

    repo.write_file("--version.txt", "hello\n");
    repo.git(&["add", "--", "--version.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":--version.txt"]), "HELLO\n");
}

/// A commit in a repo with no config file passes.
#[test]
fn no_config_passes() {
    let repo = TestRepo::empty();

    repo.write_file("README.md", "readme\n");
    repo.git(&["add", "README.md"]);

    assert_success(repo.stagelint(&[]));
}

/// A commit whose files match no configured pattern passes, saying so unless quietened.
#[test]
fn unmatched_files_pass() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("README.md", "readme\n");
    repo.git(&["add", "README.md"]);

    let output = assert_success(repo.stagelint(&[]));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("[WARN] Could not find any staged files matching configured tasks"),
        "a run that does nothing must say so"
    );

    let quiet = assert_success(repo.stagelint(&["--quiet"]));
    assert!(
        quiet.stderr.is_empty(),
        "--quiet suppresses the notice: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
}

/// A commit of only unlintable entries - symlink, deletion, submodule - passes, saying so unless quietened.
#[test]
fn unlintable_entries_pass() {
    if !file_symlinks_supported() {
        return;
    }

    let subrepo = TestRepo::empty();
    subrepo.write_file("README.md", "# Sub\n");
    subrepo.git(&["add", "README.md"]);
    subrepo.git(&["commit", "-m", "initial"]);

    let repo = TestRepo::new(&json!({"*": "false"}));

    repo.write_file("real.txt", "hello\n");
    repo.write_file("delete-me.txt", "bye\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);

    symlink_file("real.txt", &repo.root.join("link.txt")).expect("create symlink");
    repo.git(&["add", "link.txt"]);
    repo.git(&["rm", "-q", "delete-me.txt"]);
    let subrepo_str = subrepo.root.to_str().expect("path");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--force",
        subrepo_str,
        "submodule.txt",
    ]);
    // `submodule add` also stages `.gitmodules`, which is a lintable regular file.
    repo.git(&["rm", "-q", "--cached", ".gitmodules"]);

    let output = assert_success(repo.stagelint(&[]));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[WARN] Could not find any staged files")
            && !stderr.contains("matching configured tasks"),
        "the empty scope must be explained, got: {stderr}"
    );

    let quiet = assert_success(repo.stagelint(&["--quiet"]));
    assert!(
        quiet.stderr.is_empty(),
        "--quiet suppresses the notice: {}",
        String::from_utf8_lossy(&quiet.stderr)
    );
}

/// Overlapping globs never run concurrently: the later task starts only after the earlier ends.
#[test]
fn overlapping_globs_serialized_under_concurrency() {
    let repo = TestRepo::empty();

    // IndexMap, not json!: serde_json sorts keys, which would flip the order under test.
    let mut config = indexmap::IndexMap::new();
    config.insert("file.txt", sentinel(1));
    config.insert("*.txt", "sh -c 'touch marker'".to_string());
    repo.write_file(
        ".stagelint.json",
        &serde_json::to_string(&config).expect("serialize"),
    );
    repo.git(&["add", ".stagelint.json"]);
    repo.git(&["commit", "-m", "initial"]);

    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);

    let child = repo.stagelint(&[]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));

    // The first task is blocked on the sentinel; the second must not have started.
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !repo.root.join("marker").exists(),
        "overlapping task started while the earlier one was still running"
    );

    repo.release_sentinel(1);
    assert_success(child);
    assert!(repo.root.join("marker").exists());
}

/// Overlapping globs run in declaration order, not sorted-pattern order.
#[test]
fn overlapping_globs_run_in_declaration_order() {
    let repo = TestRepo::empty();

    // IndexMap, not json!: serde_json sorts keys, which would flip the order under test.
    let mut config = indexmap::IndexMap::new();
    config.insert("a.txt", replace("1", "2"));
    config.insert("*.txt", replace("2", "3"));
    repo.write_file(
        ".stagelint.json",
        &serde_json::to_string(&config).expect("serialize"),
    );
    repo.git(&["add", ".stagelint.json"]);
    repo.git(&["commit", "-m", "initial"]);

    repo.write_file("a.txt", "1\n");
    repo.git(&["add", "a.txt"]);

    assert_success(repo.stagelint(&[]));

    // Declared order: 1 -> 2 -> 3. Sorted order would give "2".
    assert_eq!(repo.read_file("a.txt"), "3\n");
    assert_eq!(repo.git(&["show", ":a.txt"]), "3\n");
}

// Submodules

/// Submodule entries (mode 160000) are not passed to the linter.
#[test]
fn submodule_not_linted() {
    let subrepo = TestRepo::empty();
    subrepo.write_file("README.md", "# Sub\n");
    subrepo.git(&["add", "README.md"]);
    subrepo.git(&["commit", "-m", "initial"]);

    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    let subrepo_str = subrepo.root.to_str().expect("path");

    // Submodule named to match *.txt; its mode-160000 entry would reach the linter if not filtered.
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--force",
        subrepo_str,
        "submodule.txt",
    ]);

    assert_success(repo.stagelint(&[]));
}

/// A submodule with new commits is not stashed: a gitlink has no file content to hide.
#[test]
fn dirty_submodule_not_stashed() {
    let subrepo = TestRepo::empty();
    subrepo.write_file("README.md", "# Sub\n");
    subrepo.git(&["add", "README.md"]);
    subrepo.git(&["commit", "-m", "initial"]);

    let repo = TestRepo::new(&json!({"*.txt": "true"}));
    let subrepo_str = subrepo.root.to_str().expect("path");
    repo.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--force",
        subrepo_str,
        "vendor",
    ]);
    repo.git(&["commit", "-m", "add submodule"]);

    // Advance the submodule so its gitlink no longer matches the index.
    repo.git(&[
        "-C",
        "vendor",
        "-c",
        "user.email=test@test.com",
        "-c",
        "user.name=Test",
        "commit",
        "--allow-empty",
        "-m",
        "advance",
    ]);

    repo.write_file("staged.txt", "hello\n");
    repo.git(&["add", "staged.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    let status = repo.git(&["status", "--short"]);
    assert!(
        status.contains("M vendor"),
        "submodule should remain dirty and untouched, got: {status}"
    );
}

// Symlinks

/// A symlinked `.git` directory is handled transparently.
#[test]
fn symlinked_git_dir() {
    if !dir_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    let git_dir = repo.root.join(".git");
    let real_dir = repo.root.join("git");
    fs::rename(&git_dir, &real_dir).expect("rename .git to git");
    symlink_dir("git", &git_dir).expect("symlink .git -> git");

    repo.write_file("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\n");
}

/// A config file that is a symlink to another path is loaded correctly.
#[test]
fn symlinked_config_file() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::empty();

    let config_str = serde_json::to_string(&json!({"*.txt": UPPERCASE})).expect("serialize config");
    repo.write_file("stagelint.config.json", &config_str);
    symlink_file("stagelint.config.json", &repo.root.join(".stagelint.json"))
        .expect("symlink config");

    repo.git(&["add", "stagelint.config.json", ".stagelint.json"]);
    repo.git(&["commit", "-m", "initial"]);

    repo.write_file("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\n");
}

/// Symlink index entries (mode 120000) are not passed to the linter.
#[test]
fn symlink_entry_not_linted() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("real.txt", "hello\n");
    symlink_file("real.txt", &repo.root.join("link.txt")).expect("create symlink");
    repo.git(&["add", "link.txt"]);

    assert_success(repo.stagelint(&[]));
}

/// The merge-back never writes through a symlink that replaced a partially-staged file.
#[test]
fn merge_does_not_write_through_symlink() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "true"]);

    // The target matches the merge base, so the linter's change would merge cleanly onto it.
    repo.write_file("victim.txt", "hello\n");
    repo.git(&["add", "victim.txt"]);
    repo.git(&["commit", "-m", "add victim"]);

    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);
    // Unstaged type change: the worktree copy becomes a symlink.
    fs::remove_file(repo.root.join("file.txt")).expect("remove file");
    symlink_file("victim.txt", &repo.root.join("file.txt")).expect("create symlink");

    let output = assert_success(repo.stagelint(&[]));

    assert!(
        String::from_utf8_lossy(&output.stderr).contains("[WARN] Changes staged but not applied"),
        "the skipped merge must be reported, got: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        repo.read_file("victim.txt"),
        "hello\n",
        "the symlink target must not be overwritten by the merge"
    );
    let meta = fs::symlink_metadata(repo.root.join("file.txt")).expect("lstat");
    assert!(
        meta.file_type().is_symlink(),
        "the type change should be preserved"
    );
}

/// Partially staged file symlink (`core.symlinks=true`): stash restores the real symlink.
#[test]
fn partial_stage_file_symlink() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "true"]);

    repo.write_file("target_a.txt", "a\n");
    repo.write_file("target_b.txt", "b\n");
    repo.write_file("target_c.txt", "c\n");
    symlink_file("target_a.txt", &repo.root.join("link.txt")).unwrap();
    repo.write_file("trigger.txt", "hello\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);

    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    fs::remove_file(repo.root.join("link.txt")).unwrap();
    symlink_file("target_b.txt", &repo.root.join("link.txt")).unwrap();
    repo.git(&["add", "link.txt"]);
    fs::remove_file(repo.root.join("link.txt")).unwrap();
    symlink_file("target_c.txt", &repo.root.join("link.txt")).unwrap();

    assert_success(repo.stagelint(&[]));

    let meta = fs::symlink_metadata(repo.root.join("link.txt")).unwrap();
    assert!(
        meta.file_type().is_symlink(),
        "link.txt should be a symlink after restore"
    );
    let target = fs::read_link(repo.root.join("link.txt")).unwrap();
    assert_eq!(target.to_str().unwrap(), "target_c.txt");

    let indexed = repo.git(&["show", ":link.txt"]);
    assert_eq!(indexed, "target_b.txt");
}

/// Partially staged directory symlink (`core.symlinks=true`): stash restores the real symlink.
#[test]
fn partial_stage_dir_symlink() {
    if !dir_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "true"]);

    fs::create_dir(repo.root.join("dir_a")).unwrap();
    fs::create_dir(repo.root.join("dir_b")).unwrap();
    fs::create_dir(repo.root.join("dir_c")).unwrap();
    symlink_dir("dir_a", &repo.root.join("link")).unwrap();
    repo.write_file("trigger.txt", "hello\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);

    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    let remove_link = || {
        #[cfg(unix)]
        fs::remove_file(repo.root.join("link")).unwrap();
        #[cfg(windows)]
        fs::remove_dir(repo.root.join("link")).unwrap();
    };
    remove_link();
    symlink_dir("dir_b", &repo.root.join("link")).unwrap();
    repo.git(&["add", "link"]);
    remove_link();
    symlink_dir("dir_c", &repo.root.join("link")).unwrap();

    assert_success(repo.stagelint(&[]));

    let meta = fs::symlink_metadata(repo.root.join("link")).unwrap();
    assert!(
        meta.file_type().is_symlink(),
        "link should be a symlink after restore"
    );
    let target = fs::read_link(repo.root.join("link")).unwrap();
    assert_eq!(target.to_str().unwrap(), "dir_c");

    assert!(
        repo.root.join("link").is_dir(),
        "link should resolve to a directory"
    );

    let indexed = repo.git(&["show", ":link"]);
    assert_eq!(indexed, "dir_b");
}

/// A partially staged symlink is hidden: commands see the staged target, not the worktree one.
#[test]
fn partial_stage_symlink_hidden_from_run() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'readlink link.txt > .git/seen' _"}));
    repo.git(&["config", "core.symlinks", "true"]);

    repo.write_file("target_a.txt", "a\n");
    repo.write_file("target_b.txt", "b\n");
    repo.write_file("trigger.txt", "hello\n");
    symlink_file("target_a.txt", &repo.root.join("link.txt")).unwrap();
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);

    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    fs::remove_file(repo.root.join("link.txt")).unwrap();
    symlink_file("target_b.txt", &repo.root.join("link.txt")).unwrap();
    repo.git(&["add", "link.txt"]);
    fs::remove_file(repo.root.join("link.txt")).unwrap();
    symlink_file("target_a.txt", &repo.root.join("link.txt")).unwrap();

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.read_file(".git/seen").trim(),
        "target_b.txt",
        "the run must see the staged symlink, not the worktree one"
    );
    let target = fs::read_link(repo.root.join("link.txt")).unwrap();
    assert_eq!(
        target.to_str().unwrap(),
        "target_a.txt",
        "the worktree symlink must be restored"
    );
    assert_eq!(repo.git(&["show", ":link.txt"]), "target_b.txt");
}

/// A staged symlink deleted from the worktree is materialized for the run, then removed again.
#[test]
fn staged_symlink_deleted_from_worktree_materialized() {
    if !file_symlinks_supported() {
        return;
    }

    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'readlink link.txt > .git/seen' _"}));
    repo.git(&["config", "core.symlinks", "true"]);

    repo.write_file("real.txt", "hello\n");
    repo.write_file("trigger.txt", "hello\n");
    repo.git(&["add", "."]);
    repo.git(&["commit", "-m", "init"]);

    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);
    symlink_file("real.txt", &repo.root.join("link.txt")).unwrap();
    repo.git(&["add", "link.txt"]);
    fs::remove_file(repo.root.join("link.txt")).unwrap();

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.read_file(".git/seen").trim(),
        "real.txt",
        "the run must see the staged symlink on disk"
    );
    assert!(
        fs::symlink_metadata(repo.root.join("link.txt")).is_err(),
        "the materialized symlink must be removed after the run"
    );
    assert_eq!(repo.git(&["show", ":link.txt"]), "real.txt");
}

/// A mode-120000 entry stored as plain text (`core.symlinks=false`) is not passed to the linter.
#[test]
fn symlink_entry_not_linted_symlinks_disabled() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("link.txt", "target.txt");
    let hash = repo.git(&["hash-object", "-w", "link.txt"]);
    let hash = hash.trim();
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{hash},link.txt"),
    ]);

    assert_success(repo.stagelint(&[]));
}

/// With `core.symlinks=false`, a dirty mode-120000 entry is stashed and restored as plain text.
#[test]
fn stash_restores_symlink_entry_symlinks_disabled() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "false"]);

    // Commit a mode-120000 entry backed by a text file (as git writes with core.symlinks=false).
    repo.write_file("link.txt", "target_a.txt");
    let hash = repo.git(&["hash-object", "-w", "link.txt"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{},link.txt", hash.trim()),
    ]);
    repo.write_file("trigger.txt", "hello\n");
    repo.git(&["add", "trigger.txt"]);
    repo.git(&["commit", "-m", "init"]);

    repo.write_file("link.txt", "target_b.txt");

    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    let meta = fs::symlink_metadata(repo.root.join("link.txt")).unwrap();
    assert!(
        !meta.file_type().is_symlink(),
        "link.txt should be a plain text file with core.symlinks=false"
    );
    assert_eq!(repo.read_file("link.txt"), "target_b.txt");
}

/// With `core.symlinks=false`, a partially staged mode-120000 entry preserves workdir and index.
#[test]
fn partial_stage_symlink_entry_symlinks_disabled() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));
    repo.git(&["config", "core.symlinks", "false"]);

    // Commit a mode-120000 entry backed by a plain text file (target_a).
    repo.write_file("link.txt", "target_a.txt");
    let hash = repo.git(&["hash-object", "-w", "link.txt"]);
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("120000,{},link.txt", hash.trim()),
    ]);
    repo.write_file("trigger.txt", "hello\n");
    repo.git(&["add", "trigger.txt"]);
    repo.git(&["commit", "-m", "init"]);

    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    // Partially stage link.txt: index = target_b, workdir = target_c (both plain text files).
    repo.write_file("link.txt", "target_b.txt");
    repo.git(&["add", "link.txt"]);
    repo.write_file("link.txt", "target_c.txt");

    assert_success(repo.stagelint(&[]));

    let meta = fs::symlink_metadata(repo.root.join("link.txt")).unwrap();
    assert!(
        !meta.file_type().is_symlink(),
        "link.txt should remain a plain text file with core.symlinks=false"
    );
    assert_eq!(repo.read_file("link.txt"), "target_c.txt");

    assert_eq!(repo.git(&["show", ":link.txt"]), "target_b.txt");
}

// Init

/// `stagelint init` installs an executable pre-commit hook; re-running leaves it unchanged.
#[test]
fn init_creates_hook_idempotently() {
    let repo = TestRepo::empty();

    assert_success(repo.stagelint(&["init"]));

    let hook_path = repo.root.join(".git/hooks/pre-commit");
    assert!(hook_path.exists(), "pre-commit hook should exist");
    let content = repo.read_file(".git/hooks/pre-commit");
    assert!(
        content.contains("stagelint"),
        "hook should contain 'stagelint', got: {content}"
    );

    // A hook without the executable bit is silently ignored by git.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&hook_path).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "hook should be executable, mode: {mode:o}"
        );
    }

    assert_success(repo.stagelint(&["init"]));
    assert_eq!(
        repo.read_file(".git/hooks/pre-commit"),
        content,
        "second init should leave the hook unchanged"
    );

    // A content-matching hook that lost its executable bit is repaired.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_success(repo.stagelint(&["init"]));
        let mode = fs::metadata(&hook_path).unwrap().permissions().mode();
        assert!(
            mode & 0o111 != 0,
            "init should restore the executable bit, mode: {mode:o}"
        );
    }
}

/// `stagelint init` refuses to overwrite a foreign hook; `--force` overrides.
#[test]
fn init_no_overwrite_without_force() {
    let repo = TestRepo::empty();

    let custom = "#!/bin/sh\necho custom\n";
    repo.write_file(".git/hooks/pre-commit", custom);

    assert_failure(repo.stagelint(&["init"]));
    assert_eq!(
        repo.read_file(".git/hooks/pre-commit"),
        custom,
        "refused init must not touch the foreign hook"
    );

    assert_success(repo.stagelint(&["init", "--force"]));

    let content = repo.read_file(".git/hooks/pre-commit");
    assert!(
        content.contains("stagelint"),
        "hook should be overwritten with stagelint, got: {content}"
    );
}

/// A hook symlink is detected even when dangling; `--force` replaces the link itself.
#[test]
fn init_replaces_hook_symlink() {
    if !file_symlinks_supported() {
        return;
    }
    let repo = TestRepo::empty();

    symlink_file("gone-target", &repo.root.join(".git/hooks/pre-commit")).expect("symlink hook");

    assert_failure(repo.stagelint(&["init"]));

    assert_success(repo.stagelint(&["init", "--force"]));
    let meta = fs::symlink_metadata(repo.root.join(".git/hooks/pre-commit")).expect("hook meta");
    assert!(meta.is_file(), "the hook should be a regular file now");
    assert!(
        !repo.root.join(".git/hooks/gone-target").exists(),
        "init must not write through the link"
    );
}

/// A symlink is a foreign hook even when its target already holds the hook text.
#[test]
fn init_rejects_matching_content_symlink() {
    if !file_symlinks_supported() {
        return;
    }
    let repo = TestRepo::empty();
    repo.write_file("outside-hook", "#!/bin/sh\nstagelint\n");

    symlink_file(
        "../../outside-hook",
        &repo.root.join(".git/hooks/pre-commit"),
    )
    .expect("symlink hook");

    assert_failure(repo.stagelint(&["init"]));
}

/// A relative `core.hooksPath` resolves against the worktree root, not the caller's cwd.
#[test]
fn init_relative_hookspath_resolves_to_root() {
    let repo = TestRepo::empty();
    repo.git(&["config", "core.hooksPath", "my-hooks"]);
    repo.write_file("sub/keep.txt", "x\n");

    let mut cmd = repo.stagelint_cmd();
    let output = cmd
        .current_dir(repo.root.join("sub"))
        .arg("init")
        .output()
        .expect("run stagelint init");
    assert!(
        output.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(repo.root.join("my-hooks/pre-commit").exists());
    assert!(!repo.root.join("sub/my-hooks/pre-commit").exists());
}

/// A binary inside the worktree is invoked relative to it, independent of `PATH`.
#[test]
fn init_uses_relative_path_in_worktree() {
    let repo = TestRepo::empty();
    // `CreateProcess` assumes `.exe`, so the copy has to keep the extension to be launchable.
    let name = format!("stagelint{}", std::env::consts::EXE_SUFFIX);
    let inside = repo.root.join("tools").join(&name);
    fs::create_dir_all(repo.root.join("tools")).expect("create tools dir");
    fs::copy(stagelint_exe(), &inside).expect("copy binary");

    assert_success(
        Command::new(&inside)
            .arg("init")
            .current_dir(&repo.root)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn stagelint"),
    );

    let hook = repo.read_file(".git/hooks/pre-commit");
    let expected = format!("'./tools/{name}'");
    assert!(hook.contains(&expected), "want {expected} in: {hook}");
}

/// Outside the worktree the hook gets what `PATH` resolves to, not the running binary.
#[test]
fn init_uses_path_entry_outside_worktree() {
    let repo = TestRepo::empty();
    let bin = tempfile::tempdir().expect("temp dir");
    // Only ever looked up, never run.
    let on_path = bin
        .path()
        .join(format!("stagelint{}", std::env::consts::EXE_SUFFIX));
    fs::copy(stagelint_exe(), &on_path).expect("copy binary");

    assert_success(
        repo.stagelint_cmd()
            .arg("init")
            .env("PATH", bin.path())
            .spawn()
            .expect("spawn stagelint"),
    );

    let hook = repo.read_file(".git/hooks/pre-commit");
    let expected = format!("'{}'", on_path.display().to_string().replace('\\', "/"));
    assert!(hook.contains(&expected), "want {expected} in: {hook}");
}

/// With nothing on `PATH`, the hook falls back to the binary that installed it.
#[test]
fn init_uses_own_path_without_path_entry() {
    let repo = TestRepo::empty();
    let empty = tempfile::tempdir().expect("temp dir");

    assert_success(
        repo.stagelint_cmd()
            .arg("init")
            .env("PATH", empty.path())
            .spawn()
            .expect("spawn stagelint"),
    );

    let hook = repo.read_file(".git/hooks/pre-commit");
    let expected = format!(
        "'{}'",
        stagelint_exe().display().to_string().replace('\\', "/")
    );
    assert!(hook.contains(&expected), "want {expected} in: {hook}");
}

// Crash recovery

/// The stash snapshots the full worktree: dirty edits and deletions survive a crash.
#[test]
fn crash_dirty_state_recoverable() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("dirty.txt", "v0\n");
    repo.write_file("user_deleted.txt", "gone\n");
    repo.git(&["add", "dirty.txt", "user_deleted.txt"]);
    repo.git(&["commit", "-m", "add files"]);
    repo.write_file("dirty.txt", "v0\nedits\n");
    fs::remove_file(repo.root.join("user_deleted.txt")).expect("delete user file");

    repo.write_file("partial.txt", "staged\n");
    repo.git(&["add", "partial.txt"]);
    repo.write_file("partial.txt", "staged\nunstaged\n");

    let mut child = repo.stagelint(&[]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));

    child.kill().unwrap();
    child.wait().unwrap();

    assert_eq!(
        repo.git(&["show", "stash@{0}:dirty.txt"]),
        "v0\nedits\n",
        "stash tree should snapshot dirty content"
    );
    assert!(
        !repo
            .git_cmd(&["show", "stash@{0}:user_deleted.txt"])
            .status
            .success(),
        "stash tree should record the user's deletion"
    );

    repo.git(&["reset", "--hard", "HEAD"]);
    repo.git(&["stash", "pop"]);

    assert_eq!(repo.read_file("dirty.txt"), "v0\nedits\n");
    assert!(!repo.root.join("user_deleted.txt").exists());
    assert_eq!(repo.read_file("partial.txt"), "staged\nunstaged\n");
}

/// Crash during `--stash tracked`: stash ref survives; `git stash pop` restores dirty files.
#[test]
fn crash_stash_tracked_recoverable() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("file.txt", "original content\n");
    repo.git(&["add", "file.txt"]);
    repo.git(&["commit", "-m", "add file"]);
    repo.write_file("file.txt", "working tree content\n");

    repo.write_file("other.txt", "staged\n");
    repo.git(&["add", "other.txt"]);

    let mut child = repo.stagelint(&["--stash", "tracked"]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    child.kill().unwrap();
    child.wait().unwrap();

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.contains("stash@{0}"),
        "stash ref should survive crash for recovery: {stash_list}"
    );

    repo.git(&["stash", "pop"]);
    assert_eq!(repo.read_file("file.txt"), "working tree content\n");
}

/// Crash during `--stash untracked`: stash ref survives; `git stash pop` restores untracked files.
#[test]
fn crash_stash_untracked_recoverable() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("staged.txt", "content\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("untracked.txt", "untracked content\n");

    let mut child = repo.stagelint(&["--stash", "untracked"]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    child.kill().unwrap();
    child.wait().unwrap();

    let stash_out = repo.git(&["stash", "list"]);
    assert!(
        stash_out.contains("stash@{0}"),
        "stash ref should survive crash for recovery: {stash_out}"
    );

    assert!(
        !repo.root.join("untracked.txt").exists(),
        "untracked file should still be hidden"
    );

    repo.git(&["stash", "pop"]);

    assert_eq!(repo.read_file("untracked.txt"), "untracked content\n");

    let ls_files = repo.git(&["ls-files", "untracked.txt"]);
    assert!(
        ls_files.is_empty(),
        "untracked.txt should not be in index after restore, got: {ls_files}"
    );

    assert_eq!(repo.git(&["show", ":staged.txt"]), "content\n");
}

/// Crash during an unstaged rename: stash ref survives; recovery restores the rename state.
#[test]
fn crash_unstaged_rename_recoverable() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("old.txt", "original\n");
    repo.git(&["add", "old.txt"]);
    repo.git(&["commit", "-m", "add old.txt"]);

    repo.write_file("old.txt", "hello world\n");
    repo.git(&["add", "old.txt"]);

    repo.rename_file("old.txt", "new.txt");

    let mut child = repo.stagelint(&[]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(
        repo.git(&["stash", "list"]).contains("stash@{0}"),
        "stash ref should exist after crash"
    );

    assert_eq!(repo.read_file("old.txt"), "hello world\n");
    assert!(repo.root.join("new.txt").exists());

    repo.git(&["reset", "--hard", "HEAD"]);
    repo.git(&["stash", "pop"]);

    assert!(!repo.root.join("old.txt").exists());
    assert_eq!(repo.read_file("new.txt"), "hello world\n");
}

/// Crash while linting a staged-then-deleted new file: recovery restores the staged content.
#[test]
fn crash_staged_deletion_recoverable() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("gone.txt", "content\n");
    repo.git(&["add", "gone.txt"]);
    fs::remove_file(repo.root.join("gone.txt")).expect("delete gone");

    let mut child = repo.stagelint(&[]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    child.kill().unwrap();
    child.wait().unwrap();

    assert!(
        repo.git(&["stash", "list"]).contains("stash@{0}"),
        "stash ref should exist after crash"
    );

    repo.git(&["reset", "--hard", "HEAD"]);
    repo.git(&["stash", "pop"]);

    assert_eq!(repo.read_file("gone.txt"), "content\n");
}

/// Crash before the first commit: stash ref survives; `git stash pop --index` restores the run.
#[test]
fn crash_empty_repo_recoverable() {
    let repo = TestRepo::empty();
    let config_str = serde_json::to_string(&json!({"*.txt": sentinel(1)})).unwrap();
    repo.write_file(".stagelint.json", &config_str);
    repo.git(&["add", ".stagelint.json"]);

    repo.write_file("staged.txt", "content\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("untracked.txt", "untracked content\n");

    let mut child = repo.stagelint(&["--stash", "untracked"]);
    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));
    child.kill().unwrap();
    child.wait().unwrap();

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.contains("stash@{0}"),
        "stash ref should survive crash for recovery: {stash_list}"
    );
    assert!(
        !repo.root.join("untracked.txt").exists(),
        "untracked file should still be hidden"
    );

    repo.git(&["stash", "pop", "--index"]);

    assert_eq!(repo.read_file("untracked.txt"), "untracked content\n");
    assert_eq!(repo.git(&["show", ":staged.txt"]), "content\n");
    assert!(
        repo.git(&["ls-files", "untracked.txt"]).is_empty(),
        "untracked.txt should not be in the index after recovery"
    );
    assert!(repo.git(&["stash", "list"]).is_empty());
}
