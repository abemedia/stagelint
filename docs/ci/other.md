---
description: Run stagelint in any CI system, checking the changed files and failing on fixes.
---

# Other CI systems

A CI job installs stagelint, runs it on the files that changed, and fails when a command changes a
file.

## Installation

Install stagelint in the job alongside the tools your config runs. The install script downloads a
prebuilt binary:

```sh
curl -fsSL https://stagelint.dev/install.sh | sh
```

See the [installation page](/installation) for the script's options, or to install stagelint through
your project's package manager instead.

## Choosing the files

[`--diff`](/cli#diff) checks the files changed since a base commit, and [`--all`](/cli#all) checks
every file in the repository. `--diff` needs the history back to the base commit, so turn off
shallow cloning.

For a pull request, the base is where its branch diverged from the target branch. CI checkouts often
fetch only the branch being built, so fetch the target branch first:

```sh
git fetch origin "+refs/heads/${TARGET_BRANCH}:refs/remotes/origin/${TARGET_BRANCH}"
stagelint --diff "origin/${TARGET_BRANCH}...HEAD"
```

For a push, the base is the commit the branch pointed to before the push. A new branch has none, and
after a force push it is not in the fetched history, so fall back to every file:

```sh
if git cat-file -e "$BEFORE_SHA^{commit}" 2>/dev/null; then
  stagelint --diff "$BEFORE_SHA...HEAD"
else
  stagelint --all
fi
```

Replace `$TARGET_BRANCH` and `$BEFORE_SHA` with the variables your CI system sets for them.

To check every file, such as on a scheduled run:

```sh
stagelint --all
```

## Failing on fixes

A command that fixes what it finds, such as `prettier --write`, exits successfully, so the job would
pass even though the pull request was not formatted. Check for changes after the run:

```sh
git diff --exit-code
```
