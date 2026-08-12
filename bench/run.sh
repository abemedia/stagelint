#!/usr/bin/env bash
#
# Comparative benchmark: stagelint vs lefthook vs nano-staged vs lint-staged.
#
#   ./run.sh
#   STAGED=10,500 RUNS=3 ./run.sh
#
set -euo pipefail

cd "$(dirname "$0")"

# Parameter lists, so every value lands in the JSON under `parameters`.
TOOLS=${TOOLS:-stagelint,lefthook,nano-staged,lint-staged}
STAGED=${STAGED:-10,100}
MODES=${MODES:-clean,partial}
RUNS=${RUNS:-10}
REPO_FILES=1000

WORK=${WORK:-"$PWD/.work"}
RESULTS="$PWD/results"

for bin in hyperfine node npm git; do
  command -v "$bin" >/dev/null || { echo "missing: $bin" >&2; exit 1; }
done

echo "==> building stagelint (release)"
cargo build --release --quiet --manifest-path ../Cargo.toml

echo "==> installing pinned competitors"
npm ci --silent

# The shipped bin is a Node launcher around the Go binary; point it at the binary itself.
ln -sf "$(node -p 'require("./node_modules/lefthook/get-exe").getExePath()')" node_modules/.bin/lefthook

# Every tool on PATH so the tool can be a parameter rather than four separate commands.
mkdir -p "$RESULTS"
export PATH="$PWD/node_modules/.bin:${PWD%/*}/target/release:$PATH"
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
  'cd "$WORK" && case {tool} in lefthook) lefthook run pre-commit --no-auto-install;; *) {tool};; esac'

rm -rf "$WORK"
node table.js "$RESULTS/bench.json" | tee "$RESULTS/table.md"
