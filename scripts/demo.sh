#!/usr/bin/env bash
#
# Run stagelint against a throwaway polyglot repo, optionally recording it for the README.
#
#   scripts/demo.sh            build the fixture and run the hook
#   scripts/demo.sh --record   the same, captured to demo.gif
#
# The fixture lives in a temp directory and is removed on exit.
set -euo pipefail

RECORD=0
case "${1:-}" in
    --record) RECORD=1 ;;
    "") ;;
    *)
        echo "usage: $(basename "$0") [--record]" >&2
        exit 2
        ;;
esac

ROOT=$(cd "$(dirname "$0")/.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

missing=0
need() {
    command -v "$1" >/dev/null || {
        printf 'missing %-14s install with: %s\n' "$1" "$2"
        missing=1
    }
}
need rustfmt "rustup component add rustfmt"
need gofmt "brew install go"
need golangci-lint "brew install golangci-lint"
need npx "brew install node"
[ "$RECORD" = 1 ] && need vhs "brew install vhs"
[ "$missing" = 0 ] || exit 1

echo "==> building stagelint"
cargo build --release --quiet --manifest-path "$ROOT/Cargo.toml"
export PATH="$ROOT/target/release:$PATH"

echo "==> installing prettier, eslint and typescript"
npm install --silent --no-save --prefix "$WORK" \
    prettier@3 eslint@10 typescript@6 typescript-eslint@8

echo "==> building fixture"
cd "$WORK"
git init -q .
git config user.name "Demo"
git config user.email demo@example.com

# `*.js` and `*.ts` match several tasks each, so those serialise while Go and Rust run alongside.
cat > .stagelint.json <<'EOF'
{
  "*.{js,ts,md,json}": "npx prettier --write",
  "*.{js,ts}": "npx eslint --fix",
  "*.ts": { "command": "npx tsc --noEmit", "pass_filenames": false },
  "*.go": [{ "command": "golangci-lint run", "pass_filenames": false }],
  "*.rs": ["rustfmt --edition 2021", { "command": "cargo clippy --quiet", "pass_filenames": false }]
}
EOF
cat > eslint.config.mjs <<'EOF'
import tseslint from "typescript-eslint";

export default tseslint.config({
  files: ["**/*.{js,ts}"],
  languageOptions: { parser: tseslint.parser },
  rules: { semi: ["error", "always"] },
});
EOF
cat > .golangci.yml <<'EOF'
version: "2"
linters:
  default: all
  exclusions:
    presets:
      - comments
    paths:
      - node_modules
EOF
printf '{"compilerOptions":{"strict":true,"noEmit":true,"skipLibCheck":true},"exclude":["node_modules"]}\n' > tsconfig.json
printf 'module demo\n\ngo 1.21\n' > go.mod
printf '[package]\nname = "demo"\nversion = "0.0.0"\nedition = "2021"\n\n[workspace]\n' > Cargo.toml
printf 'node_modules\n' > .gitignore
mkdir -p src

write_sources() {
    for i in $(seq 1 12); do
        printf 'export function web%s(%s a: number, b: number %s): number { return a + b%s }\n' \
            "$i" "$1" "$1" "$2" > "web_$i.ts"
        printf 'export function app%s(%s a, b %s) { return a + b%s }\n' \
            "$i" "$1" "$1" "$2" > "app_$i.js"
        # Clippy scales with what there is to analyse, not with file count.
        {
            for n in $(seq 1 50); do
                printf 'pub fn calc_%s_%s(%s items: &[i32] %s) -> Vec<i32> {\n' "$i" "$n" "$1" "$1"
                printf '    items.iter().filter(|v| **v %% 2 == 0).map(|v| v * %s).collect()\n' "$n"
                printf '}\n\n'
            done
        } > "src/thing_$i.rs"
        printf 'package demo\n\nfunc Thing%s(%s a int, b int %s) int { return a + b }\n' \
            "$i" "$1" "$1" > "pkg_$i.go"
    done
    for i in $(seq 1 6); do
        printf '# Notes %s\n\n- first\n- second\n' "$i" > "doc_$i.md"
    done
    # Padding, so the unstaged edit below is not adjacent to what rustfmt rewrites.
    {
        printf 'pub fn thing_0(%s a: i32, b: i32 %s) -> i32 { a + b }\n' "$1" "$1"
        printf 'pub fn helper_a() {}\npub fn helper_b() {}\n'
    } > src/thing_0.rs
    { for i in $(seq 0 12); do printf 'pub mod thing_%s;\n' "$i"; done; } > src/lib.rs
}

# Commit a tidy baseline.
write_sources "" ";"
git add .
git commit -q -m "initial commit"

# Stage the same files, misformatted.
write_sources " " ""
git add .

# Leave one file partially staged.
printf 'pub fn work_in_progress() {}\n' >> src/thing_0.rs

stagelint init > /dev/null 2>&1

echo "==> warming golangci-lint and clippy"
golangci-lint run > /dev/null 2>&1 || true
cargo clippy --quiet > /dev/null 2>&1 || true

if [ "$RECORD" = 0 ]; then
    echo "==> running"
    git commit -m "tidy up"
    exit 0
fi

echo "==> recording"
TAPE="$WORK/demo.tape"
cat > "$TAPE" <<EOF
Output "$ROOT/demo.gif"

Set Shell "bash"
Set FontSize 16
Set Width 660
Set Height 340
Set Padding 12
Set TypingSpeed 45ms

Hide
Type "cd $WORK"
Enter
Type "clear"
Enter
Show

Type \`git commit -m "tidy up"\`
Sleep 500ms
Enter
Wait+Screen@120s /files changed/
Sleep 2s
EOF
vhs "$TAPE"
printf '\nwrote %s\n' "$ROOT/demo.gif"
