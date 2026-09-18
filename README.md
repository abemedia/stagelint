# stagelint <img src="https://stagelint.dev/logo.svg" align="left" width="40" alt="">

Run commands like linters and formatters on staged git files.

- **Safe.** Partially staged files are three-way merged, so a conflicting edit never aborts your
  commit.
- **Universal.** A single binary with no runtime - the same tool in Node, Python, Go, Rust or a
  polyglot monorepo.
- **Fast.** Written in Rust, it is [5 to 30 times faster](#benchmarks) than pre-commit, lint-staged,
  Lefthook and nano-staged.

![A pre-commit run: independent globs in parallel, overlapping ones in order](https://stagelint.dev/demo.gif)

## Why stagelint?

Running formatters before a commit is easy until you stage part of a file. You stage the hunks you
want and leave the rest in your working tree - a debug line, a half-finished function. A formatter
rewrites the whole file, so its output has to be reconciled with your unstaged changes. Most tools
give up and abort the commit or, worse, commit them along with the fix.

stagelint stashes the unstaged edits, runs your commands, then three-way merges the result. What you
staged gets formatted, what you did not stays exactly where you left it, and the commit goes through
either way.

**[Read the documentation](https://stagelint.dev)**

## Getting started

### Node

```sh
npm install --save-dev @stagelint/stagelint
```

If you do not already use a hook manager, add the hook to your `prepare` script so it installs
itself for the whole team:

```json
{
  "scripts": {
    "prepare": "stagelint init"
  }
}
```

### Python

```sh
pipx install stagelint
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

### Go

```sh
go install github.com/abemedia/stagelint@latest
```

### .NET

```sh
dotnet tool install -g stagelint
```

### Homebrew

```sh
brew install abemedia/tap/stagelint
```

### Install script

Download, verify and install a prebuilt binary:

```sh
curl -fsSL https://stagelint.dev/install.sh | sh
```

It installs to `/usr/local/bin` when that is writable, otherwise to `~/.local/bin`. Set
`STAGELINT_INSTALL_DIR` to choose the directory, or `STAGELINT_VERSION` to install a specific
version.

### Other methods

See the [installation docs](https://stagelint.dev/installation) for more ways to install
**stagelint**, including APT, DNF, Pacman, Scoop, Nix and mise.

## Setting up the hook

If you already use a hook manager like pre-commit, Lefthook, or husky, call `stagelint` from its
configuration rather than running `stagelint init`. See the
[hook manager docs](https://stagelint.dev/hook-managers/) for how to set up each one.

```sh
stagelint init
```

This creates `.git/hooks/pre-commit` (or respects `core.hooksPath`). Use `--force` to overwrite an
existing hook.

Pass any CLI flag after `--` for the hook to run stagelint with:

```sh
stagelint init -- --stash tracked
```

See the [CLI reference](https://stagelint.dev/cli) for the full list of supported flags.

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

Negation patterns such as `!(*.ts)` are not supported, and match nothing rather than failing, so a
task configured with one never runs.

Commands are split using POSIX shell rules on all platforms, so quote any argument containing spaces
or backslashes.

### Monorepo support

Place config files at any level in the repo. Each staged file uses the closest config file walking
up toward the root.

### Locally installed tools

Commands resolve to the tools installed in your project, so there is no need for `npx` or `uv run`.
Any `node_modules/.bin`, `.venv/bin` (`.venv/Scripts` on Windows) or `vendor/bin` directory in the
config file's directory, or in any directory above it up to the repository root, is added to `PATH`.

## Coding agents

The same config runs over a coding agent's edits and hands failures back for the agent to fix. For
Claude Code, add this to `.claude/settings.json`:

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "stagelint --unstaged --quiet || exit 2"
          }
        ]
      }
    ]
  }
}
```

See the [agent hook docs](https://stagelint.dev/agent-hooks/) for how to set up other agents.

## Continuous integration

Check every pull request, including commits that skipped the hook. In GitHub Actions, add the
[stagelint action](https://github.com/abemedia/stagelint-action) to a workflow:

```yaml
name: stagelint

on:
  pull_request:
  push:
    branches: [main]

permissions:
  contents: read

jobs:
  stagelint:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7

      # Install the tools your stagelint config runs.

      - uses: abemedia/stagelint-action@v1
```

See the [CI docs](https://stagelint.dev/ci/) for the action's options and instructions for other CI
providers.

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
