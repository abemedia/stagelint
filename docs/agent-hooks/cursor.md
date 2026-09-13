---
description: Run linters and formatters over Cursor's edits with a stop hook in .cursor/hooks.json,
  and send failures back as a followup_message.
---

# Cursor hooks

[Download and install jq](https://jqlang.org/download/) before adding this hook, and make sure it is
on your `PATH`. The script uses it to escape stagelint's output as JSON.

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

Configure the hook in `.cursor/hooks.json` and point it at a script:

```json
{
  "version": 1,
  "hooks": {
    "stop": [{ "command": ".cursor/hooks/stagelint.sh" }]
  }
}
```

Then create `.cursor/hooks/stagelint.sh`:

```sh
#!/bin/sh
out=$(stagelint --unstaged --quiet 2>&1) && exit 0
printf '{"followup_message":%s}' "$(printf '%s' "$out" | jq -Rs .)"
```

Remember to make the script executable with `chmod +x .cursor/hooks/stagelint.sh`.

## What each part does

`stop` fires when the agent loop ends. `command` is a shell string run through a shell, and relative
paths resolve from the project root.

[`--unstaged`](/cli#unstaged) runs your commands against the working tree rather than the index,
because the agent's edits are not staged yet. Nothing is stashed and nothing is staged.
[`--quiet`](/cli#quiet) prints only the output of failed commands, which keeps the model's context
small.

A `followup_message` on stdout is submitted as the next user message, capped by `loop_limit`, which
defaults to 5. It is the only route back to the agent: exit code `2` blocks the action and any other
non-zero code is a hook failure that proceeds, but neither sends the output to the model. The
pipeline is why this needs a script rather than an inline `command`.

If every task in your config fixes what it finds, the agent does not need to hear about it, and
`command` can run `stagelint --unstaged --quiet` directly with no script at all.
