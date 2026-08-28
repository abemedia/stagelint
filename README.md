# stagelint <img src="logo.svg" align="left" width="40" alt="">

Run commands like linters and formatters on staged git files.

- **Safe.** Partially staged files are three-way merged, so a conflicting edit never aborts your
  commit.
- **Universal.** A single binary with no runtime - the same tool in Node, Python, Go, Rust or a
  polyglot monorepo.
- **Fast.** Written in Rust, it is [5 to 30 times faster](#benchmarks) than pre-commit, lint-staged,
  Lefthook and nano-staged.

![A pre-commit run: independent globs in parallel, overlapping ones in order](https://vhs.charm.sh/vhs-f7DSt1Dyw2ktDUI7fWabm.gif)

## Why stagelint?

Running formatters before a commit is easy until you stage part of a file. You add the hunks you
want and leave the rest in your working tree - a debug line, a half-finished function. A formatter
rewrites the whole file, so its output has to be reconciled with the edits you deliberately held
back. Most tools give up and abort the commit or, worse, commit them along with the fix.

stagelint stashes the unstaged edits, runs your commands, then three-way merges the result. What you
staged gets formatted, what you did not stays exactly where you left it, and the commit goes through
either way.

## Getting started

### Node

```sh
npm install --save-dev @stagelint/stagelint
```

Add the hook to your `prepare` script so it installs itself for the whole team:

```json
{
  "scripts": {
    "prepare": "stagelint init"
  }
}
```

### Python

Add it to your project:

```sh
uv add --dev stagelint
```

Or install it globally:

```sh
uv tool install stagelint
```

```sh
pipx install stagelint
```

```sh
python -m pip install --user stagelint
```

### Rust

Download a prebuilt binary using `cargo-binstall`:

```sh
cargo binstall stagelint
```

Or compile it from source:

```sh
cargo install stagelint
```

### Homebrew

```sh
brew install abemedia/tap/stagelint
```

### WinGet

```sh
winget install abemedia.stagelint
```

### Scoop

```sh
scoop bucket add abemedia https://github.com/abemedia/scoop-bucket
scoop install stagelint
```

### mise

```sh
mise use aqua:abemedia/stagelint
```

### Manual install

Download a prebuilt binary or Linux package from the
[release page](https://github.com/abemedia/stagelint/releases/latest).

## Setting up the hook

```sh
stagelint init
```

This creates `.git/hooks/pre-commit` (or respects `core.hooksPath`). Use `--force` to overwrite an
existing hook.

If you already use a hook manager like pre-commit, Lefthook, or husky, call `stagelint` from your
existing hook configuration instead.

## Configuration

Create `.stagelint.yml`, `.stagelint.yaml`, `.stagelint.json`, `.stagelint.jsonc`, or
`.stagelint.json5` in your project root. The format is a map of glob patterns to commands:

```yaml
# .stagelint.yml

# String: single command, files appended as args
'*.md': 'prettier --write'

# Object: control whether files are passed
'*.go':
  command: 'go vet ./...'
  pass_filenames: false

# Array: sequential commands, each a string or an object
'*.ts':
  - eslint --fix
  - command: 'tsc --noEmit'
    pass_filenames: false
```

Matching files are always appended as arguments unless `pass_filenames: false` is set. Commands run
from the directory of the config file that declared them, and receive absolute paths.

Commands are split using POSIX shell rules on all platforms, so quote any argument containing spaces
or backslashes.

### Monorepo support

Place config files at any level in the repo. Each staged file uses the closest config file walking
up toward the root.

## CLI flags

### `--concurrent <true|false|N>`

`true` runs every task at once, `false` runs them one at a time, and a number caps how many run
together. Tasks whose globs match the same file are always serialised regardless, in the order the
patterns are declared.

### `--continue-on-error`

By default the first failing command stops the run and cancels the rest. This runs everything to
completion and reports all failures together. The commit is still blocked, and the working tree is
still restored.

### `--diff <REVSPEC>`

Runs commands against the files changed in a revision range instead of the staged files. For
example, `main...HEAD` for everything since your branch diverged, or `HEAD~3` for the last three
commits. The commands' changes are staged, as in a normal run.

### `--unstaged`, `-u`

Runs commands against the files modified in your working tree, including untracked ones, instead of
the staged files. Nothing is hidden and nothing is staged: the commands see the working tree as it
is and their changes are left there.

### `--files <PATHS>...`

Runs commands against the given paths instead of the staged files. A path that no longer exists is
skipped rather than failing the run. Nothing is hidden and nothing is staged, as with `--unstaged`.

### `--stash <partial|tracked|untracked>`

Controls how much of your working tree is hidden while commands run, so they see the content being
committed rather than your work in progress. Each scope includes the previous, and ignored files are
never touched. Rejected with `--unstaged` and `--files`, which hide nothing.

- `partial` (default) - Only stash unstaged edits to partially staged files.
- `tracked` - Also stash every other dirty tracked file.
- `untracked` - Also stash untracked files.

Widen it when a command reads files it was not given - a type-checker or `go vet ./...` sees your
whole tree, and the default leaves your uncommitted work in place for it to trip over.

### `--quiet`, `-q`

Prints only the output of failed commands and errors: no task tree, no warnings. Cannot be combined
with `--verbose`.

### `--verbose`, `-v`

Prints the output of every command and keeps the task tree fully expanded. By default only failed
commands have their output shown, so a passing run is just the task tree.

## How it works

1. Identifies staged files and detects partially-staged ones
2. Creates a git stash (based on `--stash` scope) for crash recovery
3. Overwrites stashed files with their clean index state
4. Runs commands on the real working tree with full project context
5. Updates the git index for staged files the commands modified
6. Restores stashed files from the stash commit
7. Three-way merges the commands' changes into partially-staged files
8. Drops the stash ref

If a command fails, the working tree is restored and the commit is blocked. On crash (SIGKILL, power
loss), the stash ref survives - recover with `git stash pop`.

Paths marked `SKIP_WORKTREE` - by `git sparse-checkout` or `git update-index --skip-worktree` - are
left exactly as staged. No command sees them, and nothing on disk is staged in their place.

## Benchmarks

Each cell is `fully staged / partially staged`, measured on a 1,000-file repository with a no-op
task, on a 2019 MacBook Pro (Intel Core i9-9880H).

| Staged files | stagelint   | Lefthook      | nano-staged   | lint-staged   | pre-commit    |
| ------------ | ----------- | ------------- | ------------- | ------------- | ------------- |
| 10           | 15ms / 30ms | 152ms / 373ms | 224ms / 311ms | 437ms / 530ms | 450ms / 525ms |
| 100          | 19ms / 76ms | 163ms / 537ms | 249ms / 425ms | 455ms / 672ms | 486ms / 612ms |

Partial staging is the expensive path, and the only one where a tool has to hide your unstaged edits
and restore them afterwards. On a commit where prettier takes two seconds this is noise; it matters
on small commits and fast formatters, which is most of them. Reproduce with `bench/run.sh`.

## Thanks

- [lint-staged](https://github.com/lint-staged/lint-staged) - Inspired the configuration format and
  overall workflow.
- [git-format-staged](https://github.com/hallettj/git-format-staged) - Inspired the concept of
  formatting staged content and merging it back without blocking commits.
