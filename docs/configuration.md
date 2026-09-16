---
description: How to write a stagelint config file that maps glob patterns to commands.
---

# Configuration

Create one of these in your project root:

- `.stagelint.yml`
- `.stagelint.yaml`
- `.stagelint.json`
- `.stagelint.jsonc`
- `.stagelint.json5`

If a directory has more than one, the first in this list is used.

The format is a map of glob patterns to commands. A pattern takes a command as a string, a command
as an object, or a list of either to run in sequence:

```yaml
# .stagelint.yml
'*.md': prettier --write

'*.go':
  command: go vet ./...
  pass_filenames: false

'*.ts':
  - eslint --fix
  - command: tsc --noEmit
    pass_filenames: false
```

Matching files are appended to each command as absolute paths.

## Glob patterns

A pattern with no `/` matches by basename at any depth, so `*.ts` covers both `app.ts` and
`src/app.ts`. Any `/` anchors it: `src/*.ts` matches only files directly in `src`, and a leading `/`
or `./` pins it to the top level, so `./*.ts` excludes `src/app.ts`.

Matching is case sensitive, so `*.ts` does not match `App.TS`.

| Syntax   | Matches                                                    |
| -------- | ---------------------------------------------------------- |
| `*`      | zero or more characters, never `/`                         |
| `**`     | zero or more directories, as a whole path component        |
| `?`      | exactly one character, never `/`                           |
| `[abc]`  | one of `a`, `b` or `c`                                     |
| `[a-z]`  | one character in the range                                 |
| `[!abc]` | one character other than `a`, `b` or `c`                   |
| `{a,b}`  | either `a` or `b`, each of which may itself be a pattern   |
| `[*]`    | a literal `*`, and likewise for the other characters above |

When several patterns match the same file, their tasks run in sequence, in declaration order. That
is how you order dependent commands.

::: warning Extglobs match nothing

Extglob syntax, such as the negation `!(*.ts)` or `+(...)`, is not supported. The pattern is
accepted but matches no file, so its task never runs rather than failing. A negation is usually
better written as two overlapping patterns.

:::

If nothing matches, or no config file is found at all, stagelint prints a warning and exits
successfully.

## Task options

The object form takes two keys.

| Key              | Default  | Description                                                                                                                                            |
| ---------------- | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `command`        | required | The command line to run.                                                                                                                               |
| `pass_filenames` | `true`   | Whether matching files are appended as arguments. Set `false` for commands that scan the project themselves, such as `tsc --noEmit` or `go vet ./...`. |

## Command syntax

Commands are split into arguments using POSIX shell rules on all platforms, so quote any argument
containing spaces or backslashes.

```yaml
'*.ts': prettier --write --config "config/my prettier.json"
```

Commands run directly, not through a shell, so shell syntax has no special meaning and reaches the
command as plain arguments. Use a list to run commands in sequence, or wrap the command in
`sh -c 'tool "$@"' _` when you need a shell, where the trailing `_` becomes `$0` so the files start
at `$1` and `"$@"` covers all of them. Without it the first file is taken as `$0` and never linted.

## Monorepos

Place config files at any level in the repo. Each staged file uses the nearest config file above it.

```text
.
├── .stagelint.yml            # applies to everything without a closer config
├── packages/
│   ├── api/
│   │   └── .stagelint.yml    # applies to packages/api/**
│   └── web/
│       └── .stagelint.yml    # applies to packages/web/**
```

A config file's own directory is the base for everything in it. Patterns match relative to that
directory, and commands run with it as their working directory, so `packages/web/.stagelint.yml`
matches paths under `packages/web` and runs its commands there.

## Command resolution

Commands resolve to the tools installed in your project where available, so you do not have to
prefix them with `npx`, `uv run` or a path. Any of these directories is added to `PATH`:

- `node_modules/.bin`
- `.venv/bin`, or `.venv/Scripts` on Windows
- `vendor/bin`

stagelint looks for them in the config file's own directory, and in every directory above it up to
the repository root. So `'*.ts': eslint --fix` runs the eslint your lockfile pins rather than
whatever is installed globally. In a monorepo, a config in `packages/web` picks up its own
`node_modules/.bin` before the repository root's, so hoisted and non-hoisted layouts both work.

If no project-local copy exists, the command resolves through your normal `PATH` as usual.
