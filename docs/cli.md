---
description: Every stagelint command and option, and what each one does.
outline: [2, 3]
---

# CLI

## `stagelint` {#stagelint}

Runs the configured commands against the staged files, using the nearest config file for each.
Symlinks, submodules and skip-worktree paths are skipped unless named with `--files`.

**Usage:** `stagelint [OPTIONS]`

### `--diff <REVSPEC>` {#diff}

Runs commands against the files changed in a revision range instead of the staged files. For
example, `main...HEAD` for everything since your branch diverged, or `HEAD~3` for the last three
commits. Nothing is hidden and nothing is staged.

### `--unstaged`, `-u` {#unstaged}

Runs commands against the files with unstaged changes, including untracked files, instead of the
staged files. Nothing is hidden and nothing is staged.

### `--files <PATHS>...` {#files}

Runs commands against the given paths instead of the staged files. Nothing is hidden and nothing is
staged.

### `--all`, `-a` {#all}

Runs commands against every file that isn't ignored, instead of the staged files. Nothing is hidden
and nothing is staged.

### `--stash <partial|tracked|untracked>` {#stash}

Controls how much of your working tree is hidden while commands run, so they see the content being
committed rather than your unstaged changes. Each scope includes the previous, and ignored files are
never touched. Cannot be combined with `--diff`, `--unstaged`, `--files` or `--all`, which hide
nothing.

| Scope               | What it hides                                 |
| ------------------- | --------------------------------------------- |
| `partial` (default) | Only unstaged edits to partially staged files |
| `tracked`           | Also every other dirty tracked file           |
| `untracked`         | Also untracked files                          |

### `--concurrent <NUM|BOOL>` {#concurrent}

Runs at most this many tasks at once, `false` to run them one at a time, or `true` (default) for no
limit. Tasks whose globs match the same file are always serialised regardless, in the order the
patterns are declared.

### `--continue-on-error` {#continue-on-error}

By default the first failing command stops the run and cancels the rest. This runs everything to
completion and reports all failures together. The run still fails.

### `--max-arg-length <NUM>` {#max-arg-length}

Overrides the command-line length the system allows, in bytes, or UTF-16 characters on Windows. A
command whose arguments would exceed it is split into chunks and run once per chunk, one after
another. Commands with [`pass_filenames: false`](/configuration#task-options) are never split.

### `--quiet`, `-q` {#quiet}

Prints only the output of failed commands and errors: no task tree, no warnings.

### `--verbose`, `-v` {#verbose}

Prints the output of every command and keeps the task tree fully expanded. By default only failed
commands have their output shown.

## `stagelint init` {#init}

Installs the git pre-commit hook in `.git/hooks`, or in `core.hooksPath` if your repository sets
one.

**Usage:** `stagelint init [OPTIONS] [-- <FLAGS>...]`

### `--force` {#force}

Overwrites an existing pre-commit hook.

### `-- <FLAGS>` {#hook-flags}

Writes the flags into the hook, so every commit runs stagelint with them. Any
[`stagelint`](#stagelint) option is accepted.

```sh
stagelint init -- --stash tracked
```
