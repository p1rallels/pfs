#!/usr/bin/env bash
# Builds a throwaway toy repo in sandbox/work/toy (gitignored) carrying what
# real repos carry and worktrees don't: node_modules, a .env, a python venv
# (absolute paths baked in) and a tiny server that reads $PORT.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$here/work"
toy="$work/toy"

rm -rf "$toy"
mkdir -p "$toy"
cd "$toy"

git init -q -b main
git config user.name toy
git config user.email toy@localhost
git config commit.gpgsign false

cat > .gitignore <<'EOF'
node_modules/
.env
.venv/
*.log
EOF

cat > package.json <<'EOF'
{
  "name": "toy",
  "private": true,
  "type": "module",
  "dependencies": { "is-odd": "3.0.1" }
}
EOF

cat > server.ts <<'EOF'
const port = Number(process.env.PORT ?? 3000);
Bun.serve({ port, fetch: () => new Response(`toy on ${port}\n`) });
console.log(`listening on ${port}`);
EOF

cat > app.py <<'EOF'
import sys
print(f"toy app on {sys.executable}")
EOF

printf '# toy\n' > README.md
printf 'API_KEY=dev-not-a-secret\n' > .env
git add -A
git commit -qm "toy: initial"
printf 'step 2\n' >> README.md
git commit -qam "toy: second commit"

if command -v bun >/dev/null && bun install --silent >/dev/null 2>&1; then
  echo "node_modules: bun install"
else
  mkdir -p node_modules/is-odd
  printf 'module.exports = (n) => n %% 2 === 1;\n' > node_modules/is-odd/index.js
  echo "node_modules: fabricated (no network?)"
fi

python3 -m venv .venv
echo "venv: $(head -1 .venv/bin/pip)"
echo "toy ready at $toy"
