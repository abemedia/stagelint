---
description: Run linters and formatters over OpenCode's edits with a plugin in .opencode/plugins
  that listens for session.status idle and sends failures back as a prompt.
---

# OpenCode hooks

If stagelint is installed in your project or through a tool manager, invoke it the way that tool
does, such as `npx stagelint`, `uv run stagelint` or `mise exec -- stagelint`.

OpenCode takes a plugin rather than a config entry. Put this in `.opencode/plugins/stagelint.js`:

```javascript [.opencode/plugins/stagelint.js]
const maxAttempts = 5
const attempts = new Map()

export const Stagelint = async ({ $, client }) => ({
  event: async ({ event }) => {
    if (event.type === 'session.deleted') {
      attempts.delete(event.properties.info.id)
      return
    }

    if (event.type !== 'session.status' || event.properties.status.type !== 'idle') {
      return
    }

    const id = event.properties.sessionID
    const { data } = await client.session.get({ path: { id } })
    if (data?.parentID) {
      return
    }

    const res = await $`stagelint --unstaged --quiet`.nothrow()
    if (res.exitCode === 0) {
      attempts.delete(id)
      return
    }

    const n = (attempts.get(id) ?? 0) + 1
    if (n > maxAttempts) {
      return
    }
    attempts.set(id, n)

    await client.session.prompt({
      path: { id },
      body: { parts: [{ type: 'text', text: res.stderr.toString().trim() }] },
    })
  },
})
```

## What each part does

The plugin fires on `session.status` when the status turns idle, which is when the agent loop ends.
Subagents run in their own sessions that go idle too, so the plugin looks the session up and skips
any that has a `parentID`. A subagent has already handed its result back to the parent, and the
parent session goes idle afterwards anyway.

[`--unstaged`](/cli#unstaged) runs your commands against the working tree rather than the index,
because the agent's edits are not staged yet. Nothing is stashed and nothing is staged.
[`--quiet`](/cli#quiet) prints only the output of failed commands, which is what gets sent back as
the next message.

`$` is Bun's shell, which throws on a non-zero exit. That is exactly what a failing lint run
produces, hence `.nothrow()`. Without it the plugin throws before it can report anything.

OpenCode has no loop limit of its own. Each prompt starts a new turn, and every turn ends idle, so a
failure the agent cannot fix would prompt it forever. The plugin counts prompts per session and
stops after `maxAttempts` until the checks pass again.
