---
layout: home

hero:
  name: stagelint
  text: Lint and format staged files before every commit
  tagline:
    One fast binary that never aborts a commit over unstaged changes. Runs the same config in
    coding agent hooks and CI.
  image:
    src: /logo.svg
    alt: stagelint
  actions:
    - theme: brand
      text: Get started
      link: /introduction
    - theme: alt
      text: GitHub
      link: https://github.com/abemedia/stagelint

features:
  - icon: 🛡️
    title: Never blocked by conflicts
    details:
      When a formatter's output conflicts with your unstaged changes, stagelint three-way merges
      instead of failing the run.
  - icon: ⚡
    title: 5 to 30 times faster
    details:
      Written in Rust, stagelint is faster than pre-commit, lint-staged, Lefthook and nano-staged.
      Benchmarks are on GitHub.
  - icon: 📦
    title: One binary, any stack
    details: No runtime to install. The same tool in Node, Python, Go, Rust or a polyglot monorepo.
  - icon: 🤖
    title: Built for coding agents
    details: Run the same checks over what Claude Code, Codex or Cursor just changed, and hand
      failures back for the agent to fix.
---
