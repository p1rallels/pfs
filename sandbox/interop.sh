#!/usr/bin/env bash
# Which tools, run from a repo's main folder, treat pfs forks as more of the
# repo? Forks live outside it (<forks dir>/<repo>/<n>), so the answer should
# be none, with no setting in any tool. Builds pfs, makes a toy repo with two
# forks (a marker file to format, lint and test in main and in every fork),
# runs each tool. Cells count the fork files a tool reported:
#   from main   run in the toy's main folder (want 0)
#   in fork     run inside fork 1: the fork's own files (want > 0)
# Then the write modes, from main: fork files each one changed (want 0).
# Needs bun and uv (bunx and uvx fetch the tools). Not a pass/fail check: tools change.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$here/work/interop"
toy="$work/toy"
PFS="${PFS:-$repo/target/debug/pfs}"
export PFS_HOME="$work/pfs-home"

(cd "$repo" && cargo build -q) || exit 1
rm -rf "$work" && mkdir -p "$PFS_HOME" "$toy/src" && cd "$toy" || exit 1

eslint_rules="{ 'no-debugger': 'error', 'prefer-const': 'error' }"
git init -q -b main
git config user.name toy && git config user.email toy@localhost && git config commit.gpgsign false
printf 'node_modules/\n' >.gitignore
printf '{ "name": "toy", "type": "module", "private": true }\n' >package.json
printf '{ "compilerOptions": { "allowJs": true, "noEmit": true } }\n' >tsconfig.json
printf 'export const   x=1;\ndebugger;\nexport function probeMarker() {}\n' >src/ugly.js
printf "import test from 'node:test';\ntest('probe ' + import.meta.url, () => {});\n" >src/probe.test.js
printf 'let y = 1;\nexport { y };\n' >src/fixme.js
printf 'import sys\nimport os\nimport json\nx=1\ny: int = "s"\nprint(sys, os)\n' >src/ugly.py
printf 'def test_probe():\n    pass\n' >src/test_probe.py
printf "export default [{ rules: %s }];\n" "$eslint_rules" >eslint.config.mjs
git add -A && git commit -qm toy
"$PFS" new a >/dev/null && "$PFS" new b >/dev/null
forks=("$("$PFS" path 1)" "$("$PFS" path 2)")

TOOLS=(rg fd cm "bun test" "node --test" tsc prettier "eslint 9" "eslint 10" biome jest vitest
  black "ruff check" "ruff format" pytest mypy isort)
run_tool() {
  case $1 in
    rg) rg -l probeMarker . ;;
    fd) fd -H -E .git ugly . ;;
    cm) command -v cm >/dev/null && cm query probeMarker --no-cache ;;
    "bun test") bun test ;;
    "node --test") node --test ;;
    tsc) bunx -p typescript tsc --listFilesOnly ;;
    prettier) bunx prettier@3 --list-different . ;;
    "eslint 9") bunx eslint@9 . ;;
    "eslint 10") bunx eslint@10 . ;;
    biome) bunx @biomejs/biome@2 lint . ;;
    jest) bunx jest@30 --listTests ;;
    vitest) bunx vitest@3 list --filesOnly ;;
    black) uvx black --check . ;;
    "ruff check") uvx ruff check . ;;
    "ruff format") uvx ruff format --check . ;;
    pytest) uvx pytest --collect-only -q ;;
    mypy) uvx mypy . ;;
    isort) uvx isort --check-only . ;;
  esac
}
count() { grep -E -c "$1" || true; }
markers='(ugly\.(js|py)|probe\.test\.js|test_probe\.py)'
in_forks="forks/toy/[0-9]+/src/$markers"
cell() { (cd "$1" && run_tool "$2" 2>&1 | count "$3"); }

printf '%-12s %9s %8s\n' tool "from main" "in fork"
for t in "${TOOLS[@]}"; do
  printf '%-12s %9s %8s\n' "$t" "$(cell "$toy" "$t" "$in_forks")" "$(cell "${forks[0]}" "$t" "$markers")"
done

changed_in_forks() { for dir in "${forks[@]}"; do git -C "$dir" status --short; done | wc -l | tr -d ' '; }
reset_forks() { for dir in "${forks[@]}"; do git -C "$dir" checkout -q -- . && git -C "$dir" clean -qfd; done; }
printf '\nwrites from main: fork files changed\n'
reset_forks
for w in "bunx prettier@3 --write ." "bunx @biomejs/biome@2 check --write ." "bunx eslint@9 --fix ." \
  "uvx black ." "uvx ruff format ." "uvx isort ."; do
  read -ra cmd <<<"$w"
  "${cmd[@]}" >/dev/null 2>&1
  printf '  %-39s %s\n' "$w" "$(changed_in_forks)"
  reset_forks
done
