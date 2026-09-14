#!/usr/bin/env bash
#
# Comparative benchmark: stagelint vs prek vs hk vs lefthook vs nano-staged vs lint-staged vs
# pre-commit.
#
#   ./run.sh
#   STAGED=10,500 RUNS=3 ./run.sh
#
set -euo pipefail

cd "$(dirname "$0")"

# Parameter lists, so every value lands in the JSON under `parameters`.
TOOLS=${TOOLS:-stagelint,prek,hk,lefthook,nano-staged,lint-staged,pre-commit}
STAGED=${STAGED:-10,100}
MODES=${MODES:-clean,partial}
RUNS=${RUNS:-10}
REPO_FILES=1000

WORK=${WORK:-"$PWD/.work"}
RESULTS="$PWD/results"

for bin in mise git cargo; do
  command -v "$bin" >/dev/null || { echo "missing: $bin" >&2; exit 1; }
done

echo "==> building stagelint (release)"
cargo build --release --quiet --manifest-path ../Cargo.toml

echo "==> installing pinned competitors"
export MISE_TRUSTED_CONFIG_PATHS="$PWD"
mise install --quiet

# Every tool on PATH so the tool can be a parameter rather than separate commands. Install
# directories rather than shims, which would add a version lookup to every timed run.
mkdir -p "$RESULTS"
export PATH="${PWD%/*}/target/release:$(mise bin-paths | paste -sd: -):$PATH"
export WORK

hyperfine \
  --warmup 1 --runs "$RUNS" \
  --parameter-list tool "$TOOLS" \
  --parameter-list repo-files "$REPO_FILES" \
  --parameter-list staged "$STAGED" \
  --parameter-list mode "$MODES" \
  --setup "node fixture.js build \"\$WORK\" {repo-files} {staged} {mode}" \
  --prepare "node fixture.js stage \"\$WORK\" {staged} {mode}" \
  --command-name '{tool}' \
  --export-json "$RESULTS/bench.json" \
  'cd "$WORK" && case {tool} in lefthook) lefthook run pre-commit --no-auto-install;; hk) hk run pre-commit;; pre-commit|prek) {tool} run;; *) {tool};; esac'

rm -rf "$WORK"
node table.js "$RESULTS/bench.json" | tee "$RESULTS/table.md"
