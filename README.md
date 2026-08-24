# stagelint

Run commands like linters and formatters on staged git files.

- **Safe.** Formatter output is three-way merged into partially staged files, so a conflicting
  unstaged edit can never abort your commit.
- **Universal.** A single binary with no runtime - the same tool in Node, Python, Go, Rust or a
  polyglot monorepo.
- **Fast.** Written in Rust, it is 5 to 25 times faster than Lefthook, nano-staged and lint-staged.

## Why stagelint?

Running formatters before a commit is easy until you stage part of a file. You add the hunks you
want and leave the rest in your working tree - a debug line, a half-finished function. A formatter
rewrites the whole file, so its output has to be reconciled with the edits you deliberately held
back. Most tools give up and abort the commit or, worse, commit them along with the fix.

stagelint stashes the unstaged edits, runs your commands, then three-way merges the result. What you
staged gets formatted, what you did not stays exactly where you left it, and the commit goes through
either way.

## Benchmarks

Each cell is `fully staged / partially staged`, measured on a 1,000-file repository with a no-op
task so the figures show tool overhead rather than formatter runtime.

| Staged files | stagelint   | Lefthook      | nano-staged   | lint-staged   |
| ------------ | ----------- | ------------- | ------------- | ------------- |
| 10           | 18ms / 32ms | 162ms / 389ms | 235ms / 325ms | 450ms / 537ms |
| 100          | 20ms / 77ms | 176ms / 535ms | 258ms / 430ms | 469ms / 683ms |

Partial staging is the expensive path, and the only one where a tool has to hide your unstaged edits
and restore them afterwards. On a commit where prettier takes two seconds this is noise; it matters
on small commits and fast formatters, which is most of them. Reproduce with `bench/run.sh`.

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

`prepare` runs after `npm install`, so nobody has to remember a setup step. `stagelint init` is
idempotent, and does nothing outside a git repository.

### Everything else

Download a binary from [Releases](https://github.com/abemedia/stagelint/releases), or install from
crates.io:

```sh
cargo install stagelint
```

Then install the git pre-commit hook:

```sh
stagelint init
```

This creates `.git/hooks/pre-commit` (or respects `core.hooksPath`). Use `--force` to overwrite an
existing hook.

## Using an existing hook manager

If you use pre-commit, Lefthook, or husky, install stagelint as above, then call `stagelint` from
your existing hook configuration instead of running `stagelint init`.

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
commits.

### `--stash <partial|tracked|untracked>`

Controls how much of your working tree is hidden while commands run, so they see the content being
committed rather than your work in progress. Each scope includes the previous, and ignored files are
never touched.

- `partial` (default) - Only stash unstaged edits to partially staged files
- `tracked` - Also stash every other dirty tracked file.
- `untracked` - Also stash untracked files.

Widen it when a command reads files it was not given - a type-checker or `go vet ./...` sees your
whole tree, and the default leaves your uncommitted work in place for it to trip over.

### `--quiet`

Suppresses warnings, such as the notice printed when no task matches the staged files. Command
output and errors are unaffected.

## How it works

1. Identifies staged files and detects partially-staged ones
2. Creates a git stash (based on `--stash` scope) for crash recovery
3. Overwrites stashed files with their clean index state
4. Runs formatters on the real working tree with full project context
5. Updates the git index for staged files modified by formatters
6. Restores stashed files from the stash commit
7. Three-way merges formatting changes into partially-staged files
8. Drops the stash ref

If a linter fails, the working tree is fully restored and the commit is blocked. On crash (SIGKILL,
power loss), the stash ref survives - recover with `git stash pop`.

Paths marked `SKIP_WORKTREE` - set by `git sparse-checkout` or by
`git update-index --skip-worktree` - have no working tree file to read, so they are skipped and left
exactly as staged. Merges, rebases, and cherry-picks routinely stage such paths, so this is not
reported.

## Thanks

- [lint-staged](https://github.com/lint-staged/lint-staged) - Inspired the configuration format and
  overall workflow.
- [git-format-staged](https://github.com/hallettj/git-format-staged) - Inspired the concept of
  formatting staged content and merging it back without blocking commits.
