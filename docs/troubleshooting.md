---
description: Common problems when running stagelint, and how to fix them.
---

# Troubleshooting

## stagelint init says a hook already exists

Another tool, usually a hook manager, already owns the pre-commit hook. Call `stagelint` from that
tool's configuration instead, as described under [hook managers](/hook-managers/), or run
`stagelint init --force` to replace the hook.

## A task didn't run

Check the pattern that should have matched:

- Extglob syntax, such as `!(*.ts)`, is [not supported](/configuration#glob-patterns) and matches
  nothing. Write a negation as two overlapping patterns instead.
- A pattern containing `/` is anchored to the config file's directory, so `src/*.ts` does not match
  `src/app/main.ts`. Use `src/**/*.ts` to match at any depth.
- Each file uses only the [nearest config file](/configuration#monorepos) above it, so a config in a
  subdirectory replaces the root config for the files under it rather than adding to it.

## A skip-worktree file wasn't linted

Paths marked `SKIP_WORKTREE`, whether by `git sparse-checkout` or
`git update-index --skip-worktree`, are skipped by default and left exactly as staged: no command
sees them, and nothing on disk is staged in their place. Naming one explicitly with
[`--files`](/cli#files) lints it anyway.

## A command using `&&` or a pipe fails

Commands run without a shell, so operators such as `&&` and `|` are passed to the program as
arguments. Use a list to run commands in sequence, or wrap the command in `sh -c 'tool "$@"' _`. See
[command syntax](/configuration#command-syntax).

## A change was staged but not applied to the working tree

When a command changes a partially staged file, stagelint merges that change into your unstaged
changes. If the two conflict, the change is still staged but the working tree keeps your version.

## stagelint crashed

On a crash such as SIGKILL or power loss, the stash ref survives. List your stashes:

```sh
git stash list --format='%H %s'
```

Find the line that reads `<hash> stagelint automatic backup`, then apply it by its hash:

```sh
git stash apply --index <hash>
```
