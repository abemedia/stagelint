---
description: Run stagelint in CI so every pull request is checked, including commits that skipped
  the pre-commit hook.
---

# Continuous integration

A CI job checks every commit against the same config as the pre-commit hook, however the commit was
made.

## Why not just rely on the hook?

Anyone can skip it with `git commit --no-verify`, and commits from a web editor or a bot never run
it.

## One config for the hook and CI

A separate lint job drifts from the hook, so a commit that passed locally fails CI, or one that
skipped the hook lands unchecked. stagelint runs the same `.stagelint.yml` in both.

## Set it up

- [GitHub Actions](/ci/github-actions)
- [GitLab CI/CD](/ci/gitlab)
- [Other CI systems](/ci/other)
