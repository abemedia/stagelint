---
description:
  Run stagelint from Lefthook with a single pre-commit command, and which command options to leave
  alone so the two do not fight over file matching.
---

# Lefthook

Add one command to the `pre-commit` hook in `lefthook.yml`, alongside any commands you already have:

```yaml
# lefthook.yml
pre-commit:
  commands:
    stagelint:
      run: stagelint
```

Then install the hooks as usual:

```sh
lefthook install
```

Leave `{staged_files}` and `stage_fixed` out, since stagelint finds the staged files itself and
stages what its commands changed.

Do not set `parallel: true` on a hook that contains stagelint. It hides unstaged changes, writes to
the git index and creates a stash ref while it runs, so other commands must not run at the same
time.
