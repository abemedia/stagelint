mod helpers;

use helpers::*;
use serde_json::json;
use std::fs;
use std::process::Command;
use std::time::Duration;

// Core

/// Stagelint succeeds immediately when there are no staged files to process.
#[test]
fn no_staged_files_succeeds() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    assert_success(repo.stagelint(&[]));
}

/// The formatter's output is committed to the index and working tree.
#[test]
fn formats_staged_file() {
    let repo = TestRepo::new(&json!({"*.txt": UPPERCASE}));

    repo.write_file("hello.txt", "hello world\n");
    repo.git(&["add", "hello.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO WORLD\n");
    assert_eq!(repo.read_file("hello.txt"), "HELLO WORLD\n");
}

/// A failing linter that also modifies files: both index and working tree are restored.
#[test]
fn linter_failure_reverts_modifications() {
    let repo = TestRepo::new(&json!({"*.txt": [UPPERCASE, "false"]}));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree\n");

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.read_file("file.txt"), "working tree\n");
    assert_eq!(repo.git(&["show", ":file.txt"]), "staged\n");
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

/// stagelint fails immediately when invoked outside a git repository.
#[test]
fn fails_outside_git_repo() {
    let non_git_dir = tempfile::Builder::new()
        .prefix("stagelint-non-git")
        .tempdir()
        .expect("create temp dir");

    let output = Command::new(stagelint_exe())
        .current_dir(non_git_dir.path())
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

/// Ctrl+C mid-run cancels the linter and restores the repo with no stash left behind.
#[test]
#[cfg(unix)]
fn ctrl_c_restores_repo() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("file.txt", "staged\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree\n");

    let mut child = repo.stagelint(&[]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)),);

    assert_eq!(
        repo.read_file("file.txt"),
        "staged\n",
        "working tree should show only staged content while stash is active"
    );

    Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .expect("send SIGINT");

    let status = child.wait().expect("wait for stagelint");
    assert_eq!(
        status.code(),
        Some(1),
        "SIGINT should trigger a controlled exit, not a signal-kill"
    );

    assert_eq!(repo.git(&["show", ":file.txt"]), "staged\n");
    assert_eq!(repo.read_file("file.txt"), "working tree\n");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.is_empty(),
        "no stash entries should remain after interrupt, got: {stash_list}"
    );
}

// Stash

/// Default stash (tracked mode) hides unstaged hunks during formatting and restores them after.
#[test]
fn stash_default_hides_unstaged_changes() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line2", "FORMATTED")}));

    repo.write_file("file.txt", "line1\nline2\nline3\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "line1\nline2\nline3\nextra unstaged\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "line1\nFORMATTED\nline3\n"
    );
    assert_eq!(
        repo.read_file("file.txt"),
        "line1\nFORMATTED\nline3\nextra unstaged\n"
    );
}

/// `--stash untracked` hides untracked files so the formatter cannot see them.
#[test]
fn stash_untracked_hides_untracked_files() {
    let repo = TestRepo::new(&json!({
        "*.txt": {"command": "sh -c 'ls *.txt > manifest.txt'", "pass_filenames": false}
    }));

    repo.write_file("staged.txt", "staged content\n");
    repo.git(&["add", "staged.txt"]);
    repo.write_file("untracked.txt", "untracked content\n");

    assert_success(repo.stagelint(&["--stash", "untracked"]));

    let manifest = repo.read_file("manifest.txt");
    assert!(
        !manifest.contains("untracked.txt"),
        "untracked file should be hidden during formatting, manifest: {manifest}"
    );
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
        "rename destination should be hidden during formatting, manifest: {manifest}"
    );

    assert_eq!(repo.read_file("new.txt"), "original content\n");
    assert!(
        !repo.root.join("old.txt").exists(),
        "old.txt should remain absent after restore"
    );
}

/// A successful run leaves no committed file dirty; only the partially-staged file stays dirty.
#[test]
fn restore_does_not_dirty_index() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in *.txt; do tr a-z A-Z < \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"; done' "
    }));

    // A committed file the formatter will touch as a side effect.
    repo.write_file("committed.txt", "hello\n");
    repo.git(&["add", "committed.txt"]);
    repo.git(&["commit", "-m", "add committed"]);

    // A partially staged file: exercises the stash save/restore path.
    repo.write_file("partial.txt", "world\n");
    repo.git(&["add", "partial.txt"]);
    repo.write_file("partial.txt", "world\nextra unstaged\n");

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    // committed.txt restored (not dirty); only partial.txt stays dirty.
    let diff = repo.git(&["diff", "--name-only"]);
    assert_eq!(
        diff, "partial.txt\n",
        "only the partially-staged file should be dirty, got: {diff:?}"
    );
}

/// On linter failure, the stash ref is dropped after the working tree is restored.
#[test]
fn stash_ref_dropped_after_linter_failure() {
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

/// `--stash all` hides dirty tracked and untracked files from the formatter.
#[test]
fn stash_all_hides_dirty_and_untracked() {
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

    assert_success(repo.stagelint(&["--stash", "all"]));

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

/// `--stash tracked` hides clean tracked files that the formatter touches, then restores them.
#[test]
fn stash_tracked_restores_non_staged_files() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in *.txt; do echo MODIFIED > \"$f\"; done' "
    }));

    repo.write_file("tracked.txt", "original tracked\n");
    repo.git(&["add", "tracked.txt"]);
    repo.git(&["commit", "-m", "add tracked"]);

    repo.write_file("staged.txt", "staged content\n");
    repo.git(&["add", "staged.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    assert_eq!(
        repo.read_file("tracked.txt"),
        "original tracked\n",
        "non-staged tracked file should be restored with --stash tracked"
    );
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

    repo.write_file("hello.txt", "hello\n");
    repo.git(&["add", "hello.txt"]);
    repo.write_file("hello.txt", "hello\ngoodbye\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":hello.txt"]), "HELLO\n");
    assert_eq!(repo.read_file("hello.txt"), "HELLO\ngoodbye\n");
}

/// A stdout closed mid-run must not abort cleanup: the stash is dropped and the unstaged change survives.
#[cfg(unix)]
#[test]
fn closed_stdout_does_not_leak_stash() {
    let repo = TestRepo::new(&json!({"*.txt": "echo OUTPUT"}));

    repo.write_file("file.txt", "v1\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "v2\n");

    let mut child = repo.stagelint(&[]);
    drop(child.stdout.take()); // close stdout so the linter output hits a broken pipe
    assert!(child.wait().expect("wait").success());

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

    repo.write_file("file.txt", "hello\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "hello\nextra line\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "HELLO\n");

    assert_eq!(repo.read_file("file.txt"), "HELLO\nextra line\n");
}

/// Partially staged file: linter fails; index and working tree are both restored.
#[test]
fn partial_stage_failure_restores() {
    let repo = TestRepo::new(&json!({"*.txt": "false"}));

    repo.write_file("file.txt", "line1\nline2\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "modified_line1\nline2\n");

    assert_failure(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "line1\nline2\n");
    assert_eq!(repo.read_file("file.txt"), "modified_line1\nline2\n");
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

/// When formatter and working tree edit the same line, the working tree wins.
#[test]
fn partial_stage_conflict_workdir_wins() {
    let repo = TestRepo::new(&json!({"*.txt": replace("line1", "REPLACED")}));

    repo.write_file("file.txt", "line1\nline2\n");
    repo.git(&["add", "file.txt"]);
    // Working tree also changes line1 - conflicts with the formatter's replacement.
    repo.write_file("file.txt", "modified_line1\nline2\n");

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "REPLACED\nline2\n");
    assert_eq!(repo.read_file("file.txt"), "modified_line1\nline2\n");
}

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

// Index

/// A formatter that modifies a file that is not staged: the change must not reach the index.
#[test]
fn unstaged_file_changes_not_indexed() {
    let repo = TestRepo::new(&json!({
        "*.txt": "sh -c 'for f in *.txt; do echo MODIFIED > \"$f\"; done' "
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

/// A formatter that deletes the file it was given: stagelint must not panic or crash.
#[test]
fn formatter_deletes_staged_file() {
    let repo = TestRepo::new(&json!({"*.txt": "sh -c 'rm \"$@\"' _"}));

    repo.write_file("doomed.txt", "goodbye\n");
    repo.git(&["add", "doomed.txt"]);

    // Either error or skip gracefully - must not panic.
    let output = repo.stagelint(&[]).wait_with_output().expect("wait");
    assert!(
        !String::from_utf8_lossy(&output.stderr).contains("panicked"),
        "stagelint panicked: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A staged-deleted file is not resurrected after a successful run.
#[test]
fn staged_delete_not_resurrected() {
    let repo = TestRepo::new(&json!({"*.txt": "true"}));
    repo.write_file("to-delete.txt", "delete me\n");
    repo.git(&["add", "to-delete.txt"]);
    repo.git(&["commit", "-m", "add file to delete"]);
    repo.git(&["rm", "to-delete.txt"]);
    repo.write_file("other.txt", "content\n");
    repo.git(&["add", "other.txt"]);
    assert_success(repo.stagelint(&[]));
    assert!(
        !repo.root.join("to-delete.txt").exists(),
        "to-delete.txt should remain absent after successful run"
    );
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

/// A staged rename (old -> new): the old path must not reappear after a successful run.
#[test]
fn staged_rename_old_path_absent() {
    let repo = TestRepo::new(&json!({"*.txt": "true"}));

    repo.write_file("old-name.txt", "content\n");
    repo.git(&["add", "old-name.txt"]);
    repo.git(&["commit", "-m", "add old-name"]);

    repo.git(&["mv", "old-name.txt", "new-name.txt"]);

    repo.write_file("trigger.txt", "trigger\n");
    repo.git(&["add", "trigger.txt"]);

    assert_success(repo.stagelint(&[]));

    assert!(
        !repo.root.join("old-name.txt").exists(),
        "old-name.txt should remain absent after stagelint"
    );
    assert!(
        repo.root.join("new-name.txt").exists(),
        "new-name.txt should exist after stagelint"
    );
}

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

// Runner

/// Commands in a pipeline are executed sequentially; each receives the output of the previous.
#[test]
fn multiple_commands_sequential() {
    let repo = TestRepo::new(&json!({
        "*.txt": [replace("a", "A"), replace("b", "B")]
    }));

    repo.write_file("file.txt", "ab\n");
    repo.git(&["add", "file.txt"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":file.txt"]), "AB\n");
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

/// Without `--continue-on-error`, the pipeline stops after the first failure.
#[test]
fn default_stops_on_first_error() {
    let repo = TestRepo::new(&json!({
        "*.txt": ["false", "sh -c 'echo ran > marker.txt'"]
    }));

    repo.write_file("file.txt", "content\n");
    repo.git(&["add", "file.txt"]);

    assert_failure(repo.stagelint(&[]));
    assert!(
        !repo.root.join("marker.txt").exists(),
        "marker file should not exist without --continue-on-error"
    );
    assert_eq!(
        repo.git(&["show", ":file.txt"]),
        "content\n",
        "index should be unchanged when pipeline aborts"
    );
}

/// `--continue-on-error`: all pipeline steps run despite a failure; exit code still non-zero.
#[test]
fn continue_on_error_runs_all_on_failure() {
    let repo = TestRepo::new(&json!({
        "*.txt": ["false", "sh -c 'echo ran > marker.txt'"]
    }));

    repo.write_file("file.txt", "content\n");
    repo.git(&["add", "file.txt"]);

    assert_failure(repo.stagelint(&["--continue-on-error"]));
    assert!(
        repo.root.join("marker.txt").exists(),
        "marker file should exist with --continue-on-error (second command ran)"
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
    assert_failure(child);
}

/// A command that cannot be found returns a non-zero exit code.
#[test]
fn command_not_found_fails() {
    let repo = TestRepo::new(&json!({"*.txt": "cmd-that-does-not-exist-stagelint-test"}));

    repo.write_file("file.txt", "content\n");
    repo.git(&["add", "file.txt"]);

    assert_failure(repo.stagelint(&[]));
}

// Config

/// Glob matching: `*.txt` matches in subdirectories; `sub/*.md` only matches directly in `sub/`.
#[test]
fn glob_pattern_matching() {
    let repo = TestRepo::new(&json!({
        "*.txt": UPPERCASE,
        "sub/*.md": UPPERCASE,
    }));

    repo.write_file("root.txt", "hello\n");
    repo.git(&["add", "root.txt"]);
    repo.write_file("sub/nested.txt", "world\n");
    repo.git(&["add", "sub/nested.txt"]);
    repo.write_file("readme.md", "hello\n");
    repo.git(&["add", "readme.md"]);
    repo.write_file("sub/notes.md", "hello\n");
    repo.git(&["add", "sub/notes.md"]);

    assert_success(repo.stagelint(&[]));

    assert_eq!(repo.git(&["show", ":root.txt"]), "HELLO\n");
    assert_eq!(repo.git(&["show", ":sub/nested.txt"]), "WORLD\n");
    assert_eq!(repo.git(&["show", ":readme.md"]), "hello\n");
    assert_eq!(repo.git(&["show", ":sub/notes.md"]), "HELLO\n");
}

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

    assert_success(repo.stagelint(&["--concurrent", "false"]));

    // Declared order: 1 -> 2 -> 3. Sorted order would give "2".
    assert_eq!(repo.read_file("a.txt"), "3\n");
    assert_eq!(repo.git(&["show", ":a.txt"]), "3\n");
}

// Status

/// On-disk rename (no `git mv`): the old index entry is formatted, the rename preserved.
#[test]
fn staged_rename_preserved() {
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

/// Staged rename with failing linter: the rename state is preserved even on failure.
#[test]
fn staged_rename_failure_preserved() {
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

    // Modify the "symlink" text file on disk so it is dirty.
    repo.write_file("link.txt", "target_b.txt");

    // Stage a trigger so stagelint has something to lint.
    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    assert_success(repo.stagelint(&["--stash", "tracked"]));

    // link.txt should be restored as a plain text file, not a real symlink.
    let meta = fs::symlink_metadata(repo.root.join("link.txt")).unwrap();
    assert!(
        !meta.file_type().is_symlink(),
        "link.txt should be a plain text file with core.symlinks=false"
    );
    assert_eq!(repo.read_file("link.txt"), "target_b.txt");
}

/// With `core.symlinks=false`, a partially staged mode-120000 entry preserves workdir and index.
#[test]
fn partial_staged_symlink_entry_symlinks_disabled() {
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

    // Stage trigger.txt so stagelint has something to lint.
    repo.write_file("trigger.txt", "updated\n");
    repo.git(&["add", "trigger.txt"]);

    // Partially stage link.txt: index = target_b, workdir = target_c (both plain text files).
    repo.write_file("link.txt", "target_b.txt");
    repo.git(&["add", "link.txt"]);
    repo.write_file("link.txt", "target_c.txt");

    assert_success(repo.stagelint(&[]));

    // Workdir should be restored to target_c (3-way merge preserves workdir).
    let meta = fs::symlink_metadata(repo.root.join("link.txt")).unwrap();
    assert!(
        !meta.file_type().is_symlink(),
        "link.txt should remain a plain text file with core.symlinks=false"
    );
    assert_eq!(repo.read_file("link.txt"), "target_c.txt");

    // Index should keep the staged version (target_b).
    assert_eq!(repo.git(&["show", ":link.txt"]), "target_b.txt");
}

/// Partially staged file symlink (`core.symlinks=true`): stash restores the real symlink.
#[test]
fn partial_staged_file_symlink() {
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
fn partial_staged_dir_symlink() {
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

// Init

/// `stagelint init` creates a pre-commit hook that invokes stagelint.
#[test]
fn init_creates_pre_commit_hook() {
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
}

/// Running `stagelint init` twice succeeds; the hook is not duplicated.
#[test]
fn init_idempotent() {
    let repo = TestRepo::empty();

    assert_success(repo.stagelint(&["init"]));
    let first = repo.read_file(".git/hooks/pre-commit");

    assert_success(repo.stagelint(&["init"]));
    assert_eq!(
        repo.read_file(".git/hooks/pre-commit"),
        first,
        "second init should leave the hook unchanged"
    );

    // A content-matching hook that lost its executable bit is repaired.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let hook_path = repo.root.join(".git/hooks/pre-commit");
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

/// Run-only flags cannot be combined with the `init` subcommand.
#[test]
fn init_rejects_run_flags() {
    let repo = TestRepo::empty();

    assert_failure(repo.stagelint(&["--quiet", "init"]));
    assert_failure(repo.stagelint(&["--concurrent", "false", "init"]));
}

/// A relative `core.hooksPath` resolves against the worktree root, not the caller's cwd.
#[test]
fn init_relative_hookspath_resolves_to_root() {
    let repo = TestRepo::empty();
    repo.git(&["config", "core.hooksPath", "my-hooks"]);
    repo.write_file("sub/keep.txt", "x\n");

    let output = Command::new(stagelint_exe())
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

// Crash recovery

/// Crash during default stash: the stash ref survives for `git stash pop` recovery.
#[test]
fn crash_default_stash_recoverable() {
    let repo = TestRepo::new(&json!({"*.txt": sentinel(1)}));

    repo.write_file("file.txt", "staged content\n");
    repo.git(&["add", "file.txt"]);
    repo.write_file("file.txt", "working tree content\n");

    let mut child = repo.stagelint(&[]);

    assert!(repo.wait_sentinel(1, Duration::from_secs(10)));

    repo.write_file(".git/index.lock", "");

    child.kill().expect("kill");
    child.wait().expect("wait");

    let stash_list = repo.git(&["stash", "list"]);
    assert!(
        stash_list.contains("stash@{0}"),
        "stash ref should exist after crash for recovery: {stash_list}"
    );

    let _ = fs::remove_file(repo.root.join(".git/index.lock"));

    repo.git(&["reset", "--hard", "HEAD"]);
    repo.git(&["stash", "pop"]);

    assert_eq!(repo.read_file("file.txt"), "working tree content\n");
}

/// Crash during `--stash tracked`: stash ref survives; `git stash pop` restores dirty tracked files.
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
    child.kill().expect("kill");
    child.wait().expect("wait");

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

/// Crash during a staged rename: stash ref survives; recovery restores the on-disk rename state.
#[test]
fn crash_staged_rename_recoverable() {
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

/// Crash while formatting a staged-then-deleted new file: recovery restores the staged content.
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
