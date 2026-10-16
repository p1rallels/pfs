#!/usr/bin/env bash
# Which tools, run from a repo's main folder, treat pfs forks (.forks/<n>) as
# more of the repo? Builds pfs, makes toy repos with two forks each (a marker
# file to format, lint and test in main and in every fork), runs each tool.
# Cells count the fork files a tool reported (0 = it leaves forks alone).
#   pfs         pfs as is: /.forks/ in .git/info/exclude
#   +gitignore  '.forks/' in the repo's own .gitignore too
#   +ignores    each tool's own ignore setting (the snippets in README.md)
#   in fork     run inside fork 1 of the +ignores toy: the fork's own files (want > 0)
# Then the write modes, from main of the 'pfs' toy: fork files each one changed.
# Needs bun and uv (bunx and uvx fetch the tools). Not a pass/fail check: tools change.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$here/work/interop"
PFS="${PFS:-$repo/target/debug/pfs}"
export XDG_CONFIG_HOME="$work/config"

(cd "$repo" && cargo build -q) || exit 1
rm -rf "$work" && mkdir -p "$XDG_CONFIG_HOME"

tool_ignores() {
  printf '.forks/\n' >.prettierignore
  printf "export default [{ ignores: ['.forks/'] }, { rules: %s }];\n" "$eslint_rules" >eslint.config.mjs
  printf '{ "vcs": { "enabled": true, "clientKind": "git", "useIgnoreFile": true } }\n' >biome.json
  printf '{ "testPathIgnorePatterns": ["/node_modules/", "<rootDir>/.forks/"], "modulePathIgnorePatterns": ["<rootDir>/.forks/"] }\n' >jest.config.json
  printf "export default { test: { exclude: ['**/node_modules/**', '**/.git/**', '.forks/**'] } };\n" >vitest.config.mjs
  printf '%s\n' "[tool.black]" "extend-exclude = '/\.forks/'" "[tool.mypy]" "exclude = ['^\.forks/']" \
    "[tool.isort]" 'extend_skip = [".forks"]' >pyproject.toml
}

eslint_rules="{ 'no-debugger': 'error', 'prefer-const': 'error' }"

# toy <name>: a repo with marker files, committed, then forked twice
toy() {
  mkdir -p "$work/$1/src" && cd "$work/$1" || exit 1
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
  [ "$1" = ignores ] && tool_ignores
  git add -A && git commit -qm toy
  "$PFS" new a >/dev/null && "$PFS" new b >/dev/null
}

(toy pfs)
(toy gitignore && printf '.forks/\n' >>.gitignore)
(toy ignores)

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
in_forks="\.forks/[0-9]+/src/$markers"
own="$markers"
cell() { (cd "$work/$1" && run_tool "$2" 2>&1 | count "$3"); }

printf '%-12s %5s %11s %9s %8s\n' tool pfs +gitignore +ignores "in fork"
for t in "${TOOLS[@]}"; do
  printf '%-12s %5s %11s %9s %8s\n' "$t" \
    "$(cell pfs "$t" "$in_forks")" "$(cell gitignore "$t" "$in_forks")" \
    "$(cell ignores "$t" "$in_forks")" "$(cell ignores/.forks/1 "$t" "$own")"
done

changed_in_forks() { for n in 1 2; do git -C ".forks/$n" status --short; done | wc -l | tr -d ' '; }
reset_forks() { for n in 1 2; do git -C ".forks/$n" checkout -q -- . && git -C ".forks/$n" clean -qfd; done; }
printf '\nwrites from main (pfs toy): fork files changed\n'
cd "$work/pfs" || exit 1
for w in "bunx prettier@3 --write ." "bunx @biomejs/biome@2 check --write ." "bunx eslint@9 --fix ." \
  "uvx black ." "uvx ruff format ." "uvx isort ."; do
  read -ra cmd <<<"$w"
  "${cmd[@]}" >/dev/null 2>&1
  printf '  %-39s %s\n' "$w" "$(changed_in_forks)"
  reset_forks
done
