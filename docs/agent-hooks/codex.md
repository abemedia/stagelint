---
description: Run linters and formatters over Codex's edits with a Stop hook in .codex/hooks.json, so
  a failure becomes a continuation prompt the agent has to fix.
---

# Codex hooks

[Download and install jq](https://jqlang.org/download/) before adding this hook, and make sure it is
on your `PATH`. The loop guard uses it to read `stop_hook_active` from Codex's hook input.

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

Configure the hook in `.codex/hooks.json`, or inline in `.codex/config.toml`:

::: code-group

```json [.codex/hooks.json]
{
  "hooks": {
    "Stop": [
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

```toml [.codex/config.toml]
[[hooks.Stop]]

[[hooks.Stop.hooks]]
type = "command"
command = "jq -e .stop_hook_active >/dev/null || stagelint --unstaged --quiet || exit 2"
```

:::

Trust the project, then open `/hooks` to review and trust the hook. Codex skips it until you do, and
again whenever the hook changes.

## What each part does

`Stop` fires when the main agent finishes a turn.

[`--unstaged`](/cli#unstaged) runs your commands against the working tree rather than the index,
because the agent's edits are not staged yet. Nothing is stashed and nothing is staged.
[`--quiet`](/cli#quiet) prints only the output of failed commands, which keeps the model's context
small.

Exit 2 does not reject the turn. Codex continues and turns stderr into a new continuation prompt.

Codex has no limit of its own, so a failure the agent cannot fix would prompt it forever. It marks
the continuation turn with `stop_hook_active`, which the guard reads to stop after one retry.
