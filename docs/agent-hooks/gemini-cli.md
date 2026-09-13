---
description: Run linters and formatters over Gemini CLI's edits with an AfterAgent hook in
  .gemini/settings.json, so a failure becomes a retry turn the agent has to fix.
---

# Gemini CLI hooks

[Download and install jq](https://jqlang.org/download/) before adding this hook, and make sure it is
on your `PATH`. The loop guard uses it to read `stop_hook_active` from Gemini CLI's hook input.

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

Configure the hook in `.gemini/settings.json`:

```json
{
  "hooks": {
    "AfterAgent": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "jq -e .stop_hook_active >/dev/null || stagelint --unstaged --quiet || exit 2"
          }
        ]
      }
    ]
  }
}
```

Commit the file and the whole team gets the hook.

## What each part does

`AfterAgent` fires when the agent loop ends.

[`--unstaged`](/cli#unstaged) runs your commands against the working tree rather than the index,
because the agent's edits are not staged yet. Nothing is stashed and nothing is staged.
[`--quiet`](/cli#quiet) prints only the output of failed commands, which keeps the model's context
small.

Exit 2 rejects the response and starts a retry turn, using stderr as the prompt. Everything
stagelint prints goes to stderr, so stdout stays empty, which is what Gemini CLI requires of a hook
it does not need to parse.

A retry is bounded only by the 100 turns a prompt gets in total, so a failure the agent cannot fix
would eat into them. Gemini CLI marks the retry with `stop_hook_active`, which the guard reads to
stop after one retry.
