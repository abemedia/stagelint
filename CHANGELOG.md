# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.2](https://github.com/abemedia/stagelint/compare/v0.1.1...v0.1.2) - 2026-08-18

### Added

- add --diff to lint a revision range instead of the staged files ([#27](https://github.com/abemedia/stagelint/pull/27))

### Fixed

- protect partially staged and deleted symlinks during runs ([#26](https://github.com/abemedia/stagelint/pull/26))
- *(cli)* reject --concurrent 0 instead of treating it as unlimited ([#25](https://github.com/abemedia/stagelint/pull/25))
- *(init)* write a resolved path so the hook works without stagelint on PATH ([#23](https://github.com/abemedia/stagelint/pull/23))

## [0.1.1](https://github.com/abemedia/stagelint/compare/v0.1.0...v0.1.1) - 2026-08-13

### Fixed

- *(init)* no-op outside a git repository so npm prepare scripts pass ([#22](https://github.com/abemedia/stagelint/pull/22))

### Other

- *(npm)* publish as @stagelint/stagelint, npm blocks the bare name ([#20](https://github.com/abemedia/stagelint/pull/20))

## [0.1.0](https://github.com/abemedia/stagelint/releases/tag/v0.1.0) - 2026-08-13

### Added

- initial implementation ([#1](https://github.com/abemedia/stagelint/pull/1))
