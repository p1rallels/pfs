# pfs

forks of a folder, for you and your agents. `pfs new` makes an instant APFS clone of the repo you're in (node_modules, `.env`, uncommitted work and all) in `.forks/<n>`, on its own branch, with its own ports. macOS only.

there are no walls: a fork is a folder, and the rules (write only in your fork, commit on your branch, `$PORT`, don't push) are words in your AGENTS.md. `pfs rm` is the guard that matters: it won't delete a fork while anyone is working in it or while it has uncommitted work.

design and reasoning: [DESIGN.md](DESIGN.md).

## install

```sh
cargo install --path . --force
```

## use

```sh
pfs new add-csv                  # fork 8: .forks/8, branch pfs/8-add-csv, ports 40080-40089
pfs new --from main clean-try    # clean start at a commit (ignored files like .env stay)
pfs open 8                       # the door: your shell in fork 8, $PORT set (exit to leave)
pfs new try-b --of 8             # a copy of fork 8 as it is, uncommitted work included
pfs new clean-b --from pfs/8-add-csv  # a clean start at fork 8's latest commit
pfs ls                           # active forks: work to land, what's running, * = the one you're in
pfs run 8 cargo test             # foreground, in the fork, exit code passes through
pfs run 8 --bg bun dev           # background; pfs logs 8.1 to read it
pfs diff 8 --stat                # everything fork 8 changed, uncommitted included
pfs wait 8 9                     # block until their bg runs (agents included) have ended
pfs wait 8.1                     # just that run, and how it ended even if it already has
pfs rm 8 9                       # saves pfs/8-add-csv (and 9's branch) into main, then deletes the folders
```

inside a fork the number is optional, and every command but `new` and `rm` (which run from the main folder) works into any fork.

`pfs rm` refuses while anyone works in the fork: a process with its folder there (a sub, a door, a terminal pi, a dev server), a harness conversation mid-turn (pi's, read from its session files, so pi-web chats count too), or uncommitted work. `--force` skips the last two (a crashed conversation, work you don't want), never the first, and nothing is ever killed.

## tools that see forks

forks live in the repo, hidden from git by `.git/info/exclude`. `bun test`, `node --test`, tsc, ruff, pytest, mypy, rg, fd and cm leave them alone. prettier, eslint, biome, jest, vitest, black and isort don't: run from the main folder they format, lint and test every fork too, and `--write` / `--fix` (black and isort: by default) rewrites files inside them. tell each one once:

| tool | setting |
| --- | --- |
| prettier | `.forks/` in `.prettierignore` (or `.gitignore`) |
| eslint | a `{ ignores: [".forks/"] }` entry in `eslint.config.*` |
| biome | `"vcs": { "enabled": true, "clientKind": "git", "useIgnoreFile": true }` (then it reads `.git/info/exclude`) |
| jest | `"testPathIgnorePatterns": ["/node_modules/", "<rootDir>/.forks/"]` and `"modulePathIgnorePatterns": ["<rootDir>/.forks/"]` |
| vitest | `test.exclude: [...configDefaults.exclude, ".forks/**"]` |
| black | `extend-exclude = '/\.forks/'` under `[tool.black]` (or `.gitignore`) |
| isort | `extend_skip = [".forks"]` under `[tool.isort]` |

`bash sandbox/interop.sh` re-checks the lot, including that tools run inside a fork still see it.

## agents

an agent is a background run like any other, so any harness works:

```sh
n=$(pfs new fix-x -q)                     # -q: just the number
r=$(pfs run "$n" --bg -q pi -p "fix x")   # or claude -p, codex exec, a script. -q: just <n>.<run>
pfs wait "$r" && pfs logs "$r"            # 1 if it failed, even before you waited. pi -p's log ends with its answer
```

the rules a sub should follow are words in your repo's AGENTS.md: see the block in [skills/pfs/SKILL.md](skills/pfs/SKILL.md). the only harness-specific code is [src/harness.rs](src/harness.rs): the env var naming a harness's conversation (lineage), and how to read the conversations it files for a folder (so `rm` sees one that's still working). adding a harness is an entry there. PRs welcome.

## orchestrating with it

`skills/pfs/SKILL.md` is a pi skill for fanning work out to forks with agents and landing it back (prep, split, wait, review, rm → merge). install: `mkdir -p ~/.pi/agent/skills/pfs && cp skills/pfs/SKILL.md ~/.pi/agent/skills/pfs/`.

## test

```sh
cargo test                       # unit tests
bash sandbox/hammer.sh           # end to end in a throwaway toy repo (from a normal shell or inside a fork)
bash sandbox/interop.sh          # which formatters, linters and test runners, run from main, see forks (needs bun and uv)
```
