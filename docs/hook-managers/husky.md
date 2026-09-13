---
description: Run stagelint from husky with one line in .husky/pre-commit, including when switching
  over from lint-staged.
---

# husky

husky installs hooks but does not run tasks, so there is nothing to configure beyond the hook file.
Put the command in `.husky/pre-commit`:

```sh
stagelint
```

## Coming from lint-staged

Replace the `lint-staged` call in `.husky/pre-commit` with `stagelint`, then move your patterns out
of `package.json` into `.stagelint.yml`.

The formats line up closely, since both map a glob to a command, but negated globs are not supported
and silently match nothing. Overlapping patterns run in declaration order instead, which usually
makes the negation unnecessary. In lint-staged:

```json
{
  "!(*.ts)": "prettier --write --ignore-unknown",
  "*.ts": ["prettier --write", "eslint --fix"]
}
```

In `.stagelint.yml`:

```yaml
'*': prettier --write --ignore-unknown
'*.ts': eslint --fix
```

The [configuration page](/configuration#glob-patterns) lists the full syntax.
