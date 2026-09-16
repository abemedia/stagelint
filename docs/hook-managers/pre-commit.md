---
description: Give stagelint the pre-commit hook and let pre-commit keep the rest, or run stagelint
  as a pre-commit hook instead.
---

# pre-commit

pre-commit stashes your unstaged changes before any hook runs and restores them itself, so whichever
tool does that stashing is the one that reconciles your unstaged changes with whatever the
formatters rewrote. Give stagelint the `pre-commit` hook and pre-commit keeps every other one.

Name the hook types pre-commit should own in `.pre-commit-config.yaml`:

```yaml
default_install_hook_types: [commit-msg, pre-push]
```

Then install both:

```sh
stagelint init
pre-commit install
```

They write different files in `.git/hooks`, so neither replaces the other.

Two things to watch. pre-commit falls back to owning the `pre-commit` hook whenever the config fails
to parse, so a stray syntax error quietly hands it back. And because stagelint is not in
`.pre-commit-config.yaml`, a CI job running `pre-commit run --all-files` does not cover it and needs
its own step.

## Running stagelint under pre-commit instead

Declare stagelint as a local hook if you would rather have one config file and one CI command.
pre-commit installs it from PyPI, which ships prebuilt binaries, so there is nothing to install
yourself:

```yaml
repos:
  - repo: local
    hooks:
      - id: stagelint
        name: stagelint
        entry: stagelint --files
        language: python
        additional_dependencies: [stagelint==0.1.7]
        require_serial: true
```

Pin an exact version. pre-commit keys the hook's environment on that string and never re-resolves
it, so an unpinned or ranged spec freezes at whatever was newest the first time each machine ran the
hook, and colleagues drift onto different versions. `pre-commit autoupdate` only touches remote
repos, so bump the number by hand.

What you give up is the three-way merge. pre-commit owns the stashing in this arrangement, so a
formatter's change that conflicts with your unstaged changes is discarded rather than merged, and
the commit is blocked, exactly as it would be with any other pre-commit hook.
