_default:
    @just --list -u

alias f := fmt
alias l := lint
alias t := test
alias r := ready

# Install the pre-commit hook
init:
    cargo run -- init -- --stash untracked

# Format, lint and test before pushing
ready: fmt lint test

# Format code
fmt:
    cargo fmt --all

# Lint with clippy
lint:
    cargo clippy --all-targets -- -D warnings

# Run tests
test:
    cargo test

# Benchmark against the alternatives; override with STAGED=10,500 RUNS=3
bench:
    ./bench/run.sh
