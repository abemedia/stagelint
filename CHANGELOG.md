# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.1](https://github.com/abemedia/stagelint/compare/v0.2.0...v0.2.1) - 2026-09-19

### Other

- *(deps)* bump indexmap from 2.14.1 to 2.14.2 ([#97](https://github.com/abemedia/stagelint/pull/97))
- support installing via go install ([#104](https://github.com/abemedia/stagelint/pull/104))
- *(deps)* bump clap from 4.6.6 to 4.6.7 ([#105](https://github.com/abemedia/stagelint/pull/105))
- *(deps)* bump serde_with from 3.22.0 to 3.23.0 ([#96](https://github.com/abemedia/stagelint/pull/96))
- *(deps)* bump console from 0.16.4 to 0.16.6 ([#95](https://github.com/abemedia/stagelint/pull/95))
- publish .NET tool packages to NuGet ([#103](https://github.com/abemedia/stagelint/pull/103))
- publish Linux packages via kubri ([#102](https://github.com/abemedia/stagelint/pull/102))
- retry spawning the copied binary while another test holds its write fd ([#99](https://github.com/abemedia/stagelint/pull/99))
- add demo gif to introduction and serve it from the site ([#98](https://github.com/abemedia/stagelint/pull/98))
- package only source, tests, license and readme ([#90](https://github.com/abemedia/stagelint/pull/90))
- add code block titles and icons ([#89](https://github.com/abemedia/stagelint/pull/89))
- fill pinned versions from Cargo.toml at build time ([#87](https://github.com/abemedia/stagelint/pull/87))

## [0.2.0](https://github.com/abemedia/stagelint/compare/v0.1.7...v0.2.0) - 2026-09-16

### Added

- [**breaking**] pass --files paths to commands as given ([#85](https://github.com/abemedia/stagelint/pull/85))
- [**breaking**] disable stash and stage when running with --diff ([#84](https://github.com/abemedia/stagelint/pull/84))
- add install script for prebuilt binaries ([#82](https://github.com/abemedia/stagelint/pull/82))

### Other

- website ([#86](https://github.com/abemedia/stagelint/pull/86))
- *(install.sh)* tweak architecture detection on Apple Silicon Macs ([#83](https://github.com/abemedia/stagelint/pull/83))
- publish to NUR and attest release artifacts ([#79](https://github.com/abemedia/stagelint/pull/79))

## [0.1.7](https://github.com/abemedia/stagelint/compare/v0.1.6...v0.1.7) - 2026-09-14

### Added

- *(cli)* add --all to lint every tracked and untracked file ([#78](https://github.com/abemedia/stagelint/pull/78))

### Fixed

- *(config)* reject invalid commands in patterns that match no files ([#75](https://github.com/abemedia/stagelint/pull/75))
- *(config)* anchor patterns with a leading / or ./ ([#70](https://github.com/abemedia/stagelint/pull/70))

### Other

- *(status)* skip the HEAD tree diff for --unstaged ([#77](https://github.com/abemedia/stagelint/pull/77))
- simplify stash, index and status helpers ([#76](https://github.com/abemedia/stagelint/pull/76))
- add linked worktrees integration test ([#73](https://github.com/abemedia/stagelint/pull/73))
- invalidate only changed paths in the cache tree ([#72](https://github.com/abemedia/stagelint/pull/72))
- add dprint and tombi formatters ([#68](https://github.com/abemedia/stagelint/pull/68))

## [0.1.6](https://github.com/abemedia/stagelint/compare/v0.1.5...v0.1.6) - 2026-09-10

### Added

- *(runner)* split commands whose arguments exceed the system limit ([#63](https://github.com/abemedia/stagelint/pull/63))

### Other

- *(deps)* bump libc from 0.2.186 to 0.2.189 ([#67](https://github.com/abemedia/stagelint/pull/67))

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
