---
description: What stagelint is, what it can do, and where to start in the docs.
---

# Introduction

stagelint runs commands such as linters, formatters and type checkers on the files you are about to
commit, and stages what they fix. It runs from its own pre-commit hook, from your hook manager, from
a coding agent's hooks, or in CI.

## Features

- Partially staged files are three-way merged, so a conflict with your unstaged changes never blocks
  the commit.
- Runs [5 to 30 times faster](https://github.com/abemedia/stagelint#benchmarks) than other
  pre-commit tools.
- A single binary with no runtime, installed through the [package manager](/installation) you
  already use.
- Prefers [tools installed in your project](/configuration#command-resolution) over global ones, so
  there is no need for `npx` or `uv run`.
- Supports [monorepos](/configuration#monorepos) with nested config files.
- Plugs into [coding agents](/agent-hooks/) such as Claude Code, Codex and Cursor, so they fix
  failing checks before handing work back to you.
- Runs in [CI](/ci/), checking only the files a pull request or push changed.

## Where to start

- Install stagelint: [Installation](/installation)
- Set up a pre-commit hook: [Getting started](/getting-started)
- Add it to a hook manager you already use: [Hook managers](/hook-managers/)
- Check a coding agent's edits: [Agent hooks](/agent-hooks/)
- Check pull requests in CI: [Continuous integration](/ci/)
- Match files to commands: [Configuration](/configuration)
