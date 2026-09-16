---
description: Run linters and formatters over GitHub Copilot CLI's edits with an agentStop hook,
  using a decision block to force another agent turn.
---

# GitHub Copilot CLI hooks

[Download and install jq](https://jqlang.org/download/) before adding this hook, and make sure it is
on your `PATH`. The bash command uses it to escape stagelint's output as JSON.

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

Configure the hook in `.github/hooks/stagelint.json`:

```json
{
  "version": 1,
  "hooks": {
    "agentStop": [
      {
        "type": "command",
        "bash": "out=$(stagelint --unstaged --quiet 2>&1) || printf '%s' \"$out\" | jq -Rs '{decision:\"block\",reason:.}'",
        "powershell": "$out = stagelint --unstaged --quiet 2>&1 | Out-String; if ($LASTEXITCODE -ne 0) { @{decision='block'; reason=$out} | ConvertTo-Json -Compress }"
      }
    ]
  }
}
```

## What each part does

`agentStop` fires when the main agent finishes a turn.

[`--unstaged`](/cli#unstaged) runs your commands against the working tree rather than the index,
because the agent's edits are not staged yet. Nothing is stashed and nothing is staged.
[`--quiet`](/cli#quiet) prints only the output of failed commands, which keeps the model's context
small.

`decision: "block"` on stdout is the only route back to the model, and forces another agent turn
using `reason` as the prompt. No exit code reaches it: exit code 2 shows stderr to you as a warning,
any other code is only logged, and the turn ends either way.

After 8 consecutive blocks Copilot ends the turn anyway, so a failure the agent cannot fix does not
loop forever.

If every task in your config fixes what it finds, the agent does not need to hear about it, and a
plain `"command": "stagelint --unstaged --quiet"` is enough.
