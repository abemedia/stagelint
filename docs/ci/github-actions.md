---
description: How to run stagelint in GitHub Actions.
---

# GitHub Actions

Add a workflow to `.github/workflows/stagelint.yml` that runs the
[stagelint action](https://github.com/abemedia/stagelint-action):

```yaml
name: stagelint

on:
  pull_request:
  merge_group:
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

The action installs stagelint, runs it on the
[files the pull request or push changed](#files-checked), and fails the job when a command fails or
changes a file.

## Inputs

| Input             | Default                         | Description                                     |
| ----------------- | ------------------------------- | ----------------------------------------------- |
| `version`         | `latest`                        | Version or semver range of stagelint to install |
| `args`            | [Changed files](#files-checked) | Arguments to run stagelint with                 |
| `workdir`         | `.`                             | Directory to run stagelint in                   |
| `install-only`    | `false`                         | Only install stagelint and add it to `PATH`     |
| `fail-on-changes` | `true`                          | Fail when a command changed a file              |
| `github-token`    | `${{ github.token }}`           | Token used to look up stagelint releases        |

## Outputs

| Output    | Description                     |
| --------- | ------------------------------- |
| `version` | The installed stagelint version |

## Files checked

Without `args`, the action checks the files changed by the event that triggered the workflow:

| Event          | Files                                                                          |
| -------------- | ------------------------------------------------------------------------------ |
| `pull_request` | Changed since the pull request's branch diverged from its base                 |
| `merge_group`  | Changed since the merge queue's base commit                                    |
| `push`         | Changed since the previous commit on the branch, or every file on a new branch |
| Anything else  | Every file                                                                     |

If the base commit cannot be fetched, such as after a force push, every file is checked and a
warning is logged.

## Fixing pull requests with autofix.ci

[autofix.ci](https://autofix.ci) commits the fixes back to the pull request instead of failing the
job, including on pull requests from forks. It needs its GitHub App installed, and the workflow must
be named `autofix.ci`:

```yaml
name: autofix.ci

on: pull_request

permissions:
  contents: read

jobs:
  autofix:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7

      # Install the tools your stagelint config runs.

      - uses: abemedia/stagelint-action@v1
        with:
          fail-on-changes: false

      - uses: autofix-ci/action@v1
```
