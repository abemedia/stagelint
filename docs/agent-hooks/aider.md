---
description: Run linters and formatters over aider's edits with one line of .aider.conf.yml.
  Auto-lint is on by default and feeds failures back to the model.
---

# aider hooks

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

aider's auto-lint is on by default and appends the edited filenames to `lint-cmd`, so one line of
`.aider.conf.yml` is enough:

```yaml
lint-cmd: stagelint --quiet --files
```

## What each part does

[`--files`](/cli#files) takes the filenames aider appends, so stagelint lints exactly the files
aider edited. [`--quiet`](/cli#quiet) prints only the output of failed commands, which keeps the
model's context small.

Nothing extra is needed to feed failures back. aider's auto-lint already takes a failing `lint-cmd`
and asks the model to fix it, so a reporting task such as a type-checker reaches the model too.
