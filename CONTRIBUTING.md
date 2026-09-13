# Contributing

## Prerequisites

- [Rust](https://rustup.rs)
- [just](https://just.systems/man/en/installation.html)
- [dprint](https://dprint.dev/install/)
- [tombi](https://tombi-toml.github.io/tombi/docs/installation)

The pre-commit hook runs dprint and tombi, so a commit fails until both are installed.

## Workflow

Install the pre-commit hook once after cloning:

```sh
just init
```

Format, lint and test before pushing:

```sh
just ready
```
