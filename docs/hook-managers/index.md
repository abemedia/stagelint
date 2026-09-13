---
description: Keep the hook manager you already use and call stagelint from your existing
  configuration instead of replacing it.
---

# Hook managers

You do not have to choose between stagelint and the hook manager you already use. Call `stagelint`
from your existing configuration and the manager keeps orchestrating your hooks while stagelint
handles what runs over your staged files.

Each manager wires it up differently, so pick yours:

- [pre-commit](/hook-managers/pre-commit)
- [Lefthook](/hook-managers/lefthook)
- [husky](/hook-managers/husky)
