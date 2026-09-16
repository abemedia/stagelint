---
description: Set up a working pre-commit hook that runs your linters and formatters over staged
  files.
---

# Getting started

This page helps you set up a pre-commit hook that runs your linters and formatters, and run the same
checks from your coding agent and in CI.

Before you begin, [install stagelint](/installation).

## Write a config

Create `.stagelint.yml` in your project root. The format is a map of glob patterns to commands:

```yaml
'*': prettier --write --ignore-unknown
'*.ts':
  - eslint --fix
  - command: tsc --noEmit
    pass_filenames: false
```

Matching files are appended to each command as absolute paths, except where `pass_filenames: false`
is set. When a file matches several patterns, their commands run in the order the patterns are
declared. The [configuration page](/configuration) covers every option.

## Run it by hand

Run `stagelint` by hand to try your config before you set up a hook. `stagelint -a` is a good first
run: it checks every file, so issues in existing code surface now rather than in your first commit.

| Command                        | Runs against                              |
| ------------------------------ | ----------------------------------------- |
| `stagelint`                    | Staged files                              |
| `stagelint -a`                 | All non-ignored files in the working tree |
| `stagelint -u`                 | Modified and untracked files              |
| `stagelint --diff main...HEAD` | Files changed since your branch diverged  |

If you installed it into your project or through a tool manager, invoke it the way that tool does,
such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

## Set up the git hook

If you already use a [hook manager](/hook-managers/), skip this step and call `stagelint` from its
configuration instead.

```sh
stagelint init
```

This creates `.git/hooks/pre-commit`, or respects `core.hooksPath` if your repository sets one. See
the [CLI page](/cli) for the options `init` accepts.

`.git/hooks` is not tracked by git, so this only sets up your local clone. Run `stagelint init` from
something every contributor already runs: a `prepare` script in `package.json`, a Makefile target,
or whatever bootstrap task you have.

## Set up an agent hook

The same config runs over a coding agent's edits, so anything that fails goes back to the agent
rather than landing on you at review. Every agent takes a different file, so see
[agent hooks](/agent-hooks/) for the one you use.

## Set up CI

The git hook only runs in clones that ran `stagelint init`, and anyone can skip it. Run the same
config in CI to check every pull request, however its commits were made. See
[continuous integration](/ci/) for the CI system you use.

## Where to go next

- [Configuration](/configuration) to learn how patterns, configs and commands are resolved
- [CLI](/cli) to look up a command or option
- [Troubleshooting](/troubleshooting) to fix a problem
