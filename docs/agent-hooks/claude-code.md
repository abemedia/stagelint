---
description: Run linters and formatters over Claude Code's edits with a Stop hook in
  .claude/settings.json, so failures go back to the model instead of landing on you at review.
---

# Claude Code hooks

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

Configure the hook in `.claude/settings.json`:

```json [.claude/settings.json]
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "stagelint --unstaged --quiet || exit 2"
          }
        ]
      }
    ]
  }
}
```

Commit the file and the whole team gets the hook.

## What each part does

`Stop` fires when the main agent finishes a turn.

[`--unstaged`](/cli#unstaged) runs your commands against the working tree rather than the index,
because the agent's edits are not staged yet. Nothing is stashed and nothing is staged.
[`--quiet`](/cli#quiet) prints only the output of failed commands, which keeps the model's context
small.

Exit code 2 is the important one. Claude Code treats it as a blocking error and feeds stderr back to
the model as a correction, so a failure becomes something the agent fixes in the same session. Any
other non-zero code is surfaced to you rather than to the model. If the agent cannot fix a failure,
Claude Code stops blocking after 8 attempts in a row, so the session never loops forever.

## Scoping it to a single machine

`.claude/settings.json` is meant to be committed. To try the hook without committing it, put the
same block in `.claude/settings.local.json` instead, which Claude Code reads for per-user overrides.
