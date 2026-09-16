---
description: Instructions in AGENTS.md are advisory and a coding agent can ignore them. A hook runs
  every turn and hands failures back to the model to fix.
---

# Agent hooks

An agent hook moves your checks to the moment the agent stops typing, while a failure is still the
agent's problem rather than yours.

## Why not just put the rules in AGENTS.md?

Because the model decides whether to follow them. Written instructions are advisory. They compete
for attention with everything else in the context window, and adherence gets worse the longer the
session runs, precisely when you are paying least attention.

A hook runs whether or not the model remembered, and hands the failure back in terms it has to act
on.

## One standard for the agent and the commit

Wiring linters into your agent separately from your pre-commit hook leaves you maintaining two sets
of rules. They drift, and the failure mode is the annoying one: the agent's work passes its own
checks and then fails yours at the moment you try to commit it.

stagelint reads the same `.stagelint.yml` in both places, so the agent is held to exactly what the
commit is about to demand.

## Set it up

- [Claude Code](/agent-hooks/claude-code)
- [Codex](/agent-hooks/codex)
- [GitHub Copilot CLI](/agent-hooks/github-copilot-cli)
- [Gemini CLI](/agent-hooks/gemini-cli)
- [Cursor](/agent-hooks/cursor)
- [aider](/agent-hooks/aider)
- [OpenCode](/agent-hooks/opencode)
