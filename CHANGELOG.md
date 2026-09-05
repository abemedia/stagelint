# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.5](https://github.com/abemedia/stagelint/compare/v0.1.4...v0.1.5) - 2026-09-05

### Added

- *(runner)* add project-local tool directories to PATH ([#60](https://github.com/abemedia/stagelint/pull/60))

## [0.1.4](https://github.com/abemedia/stagelint/compare/v0.1.3...v0.1.4) - 2026-08-28

### Added

- *(init)* support run flags ([#58](https://github.com/abemedia/stagelint/pull/58))
- *(cli)* add short flags for --unstaged, --quiet and --verbose ([#55](https://github.com/abemedia/stagelint/pull/55))
- *(cli)* add --unstaged and --files sources ([#50](https://github.com/abemedia/stagelint/pull/50))

### Fixed

- *(stash)* create a stash before the first commit ([#53](https://github.com/abemedia/stagelint/pull/53))
- *(runner)* cancel commands that close their own output ([#52](https://github.com/abemedia/stagelint/pull/52))
- *(report)* hide the cursor while the task tree is drawn ([#49](https://github.com/abemedia/stagelint/pull/49))

### Other

- add stagelint config and justfile ([#57](https://github.com/abemedia/stagelint/pull/57))
- *(readme)* document coding agent hooks and negation patterns, remove WinGet install ([#56](https://github.com/abemedia/stagelint/pull/56))
- *(readme)* document --unstaged and --files ([#51](https://github.com/abemedia/stagelint/pull/51))
- retry the flaky exiting-task race ([#54](https://github.com/abemedia/stagelint/pull/54))
- *(readme)* add logo and mise install ([#48](https://github.com/abemedia/stagelint/pull/48))
- add pre-commit to the comparative benchmark ([#47](https://github.com/abemedia/stagelint/pull/47))
- *(readme)* document new install channels, tweak copy ([#42](https://github.com/abemedia/stagelint/pull/42))

## [0.1.3](https://github.com/abemedia/stagelint/compare/v0.1.2...v0.1.3) - 2026-08-26

### Fixed

- exclude skip-worktree paths from --diff scope ([#39](https://github.com/abemedia/stagelint/pull/39))

### Other

- synchronise the exiting-task race with fifos ([#44](https://github.com/abemedia/stagelint/pull/44))
- *(deps)* bump yaml_serde from 0.10.6 to 0.10.7 ([#43](https://github.com/abemedia/stagelint/pull/43))
- use mimalloc as the global allocator on musl builds ([#41](https://github.com/abemedia/stagelint/pull/41))
- release with GoReleaser and maturin ([#40](https://github.com/abemedia/stagelint/pull/40))
- *(readme)* add demo gif, move and re-run benchmarks, tidy copy ([#35](https://github.com/abemedia/stagelint/pull/35))

## [0.1.2](https://github.com/abemedia/stagelint/compare/v0.1.1...v0.1.2) - 2026-08-24

### Added

- report progress as a live task tree, add --verbose flag, refactor runner ([#28](https://github.com/abemedia/stagelint/pull/28))
- add --diff to lint a revision range instead of the staged files ([#27](https://github.com/abemedia/stagelint/pull/27))

### Fixed

- protect partially staged and deleted symlinks during runs ([#26](https://github.com/abemedia/stagelint/pull/26))
- *(cli)* reject --concurrent 0 instead of treating it as unlimited ([#25](https://github.com/abemedia/stagelint/pull/25))
- *(init)* write a resolved path so the hook works without stagelint on PATH ([#23](https://github.com/abemedia/stagelint/pull/23))

### Other

- *(deps)* bump gix from 0.86.0 to 0.87.1 ([#32](https://github.com/abemedia/stagelint/pull/32))
- *(deps)* bump serde_with from 3.21.0 to 3.22.0 ([#30](https://github.com/abemedia/stagelint/pull/30))
- *(deps)* bump yaml_serde from 0.10.4 to 0.10.6 ([#31](https://github.com/abemedia/stagelint/pull/31))
- *(readme)* add --diff to the options reference ([#29](https://github.com/abemedia/stagelint/pull/29))

## [0.1.1](https://github.com/abemedia/stagelint/compare/v0.1.0...v0.1.1) - 2026-08-13

### Fixed

- *(init)* no-op outside a git repository so npm prepare scripts pass ([#22](https://github.com/abemedia/stagelint/pull/22))

### Other

- *(npm)* publish as @stagelint/stagelint, npm blocks the bare name ([#20](https://github.com/abemedia/stagelint/pull/20))

## [0.1.0](https://github.com/abemedia/stagelint/releases/tag/v0.1.0) - 2026-08-13

### Added

- initial implementation ([#1](https://github.com/abemedia/stagelint/pull/1))
