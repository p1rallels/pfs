# pfs

forks of a folder, for you and your agents. `pfs new` makes an instant APFS clone of the repo you're in (node_modules, `.env`, uncommitted work and all) in `~/.pfs/forks/<repo>/<n>`, on its own branch, with its own ports. macOS only.

there are no walls: a fork is a folder, and the rules (write only in your fork, commit on your branch, `$PORT`, don't push) are words in your AGENTS.md. `pfs rm` is the guard that matters: it won't delete a fork while anyone is working in it or while it has uncommitted work.

design and reasoning: [DESIGN.md](DESIGN.md).

## install

```sh
cargo install --path . --force
```

## use

```sh
pfs new add-csv                  # fork 8: ~/.pfs/forks/<repo>/8, branch pfs/8-add-csv, ports 40080-40089
pfs path 8                       # its folder, for cd "$(pfs path 8)" and editors
pfs new --from main clean-try    # clean start at a commit (ignored files like .env stay)
pfs open 8                       # the door: your shell in fork 8, $PORT set (exit to leave)
pfs new try-b --of 8             # a copy of fork 8 as it is, uncommitted work included
pfs new clean-b --from pfs/8-add-csv  # a clean start at fork 8's latest commit
pfs ls                           # active forks: work to land, what's running, * = the one you're in
                                 # (outside any repo: every repo's forks, and the ones whose repo is gone)
pfs run 8 cargo test             # foreground, in the fork, exit code passes through
pfs run 8 --bg bun dev           # background; pfs logs 8.1 to read it
pfs diff 8 --stat                # everything fork 8 changed, uncommitted included
pfs wait 8 9                     # block until their bg runs (agents included) have ended
pfs wait 8.1                     # just that run, and how it ended even if it already has
pfs rm 8 9                       # saves pfs/8-add-csv (and 9's branch) into main, then deletes the folders
```

inside a fork the number is optional, and every command but `new` and `rm` (which run from the main folder) works into any fork.

`pfs rm` refuses while anyone works in the fork: a process with its folder there (a sub, a door, a terminal pi, a dev server), a harness conversation mid-turn (pi's, read from its session files, so pi-web chats count too), or uncommitted work. `--force` skips the last two (a crashed conversation, work you don't want), never the first, and nothing is ever killed.

## where forks live

outside the repo, so nothing you run in the main folder sees them: formatters, linters and test runners (`--write` and `--fix` included), docker's build context, tar, rsync, `git clean`. no tool needs a setting.

```
~/.pfs/                    $PFS_HOME (set it to move everything)
  config.toml              [ports], and [forks] dir = "~/somewhere" to put forks elsewhere
  forks/pfs-cli/           one folder per repo, private (0700)
    .main, README.md       whose forks these are, and "remove with pfs rm, never by hand"
    8/                     fork 8
<repo>/.git/pfs/           the ledger, and which folder holds this repo's forks
```

- **same disk:** clones only work within one disk, so the forks folder has to be on the repo's. for a repo on another disk, set `[forks] dir` to a folder there.
- **a repo keeps its folder:** `[forks] dir` only places a repo's first forks. move a repo and its next pfs command finds its forks again.
- **a deleted repo leaves its forks behind:** `pfs ls` outside any repo lists them with their work, and how to keep or drop it. pfs never deletes them.
- **from pfs 0.3** (forks in `<repo>/.forks/`): land those forks with 0.3's `pfs rm` first, since the new pfs refuses a repo that still has them. the ledger moves by itself. the config moves by hand, from `~/.config/pfs/config.toml` to `~/.pfs/config.toml`.

`bash sandbox/interop.sh` checks that 18 formatters, linters and test runners see no forks from main, and still work inside one.

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
bash sandbox/interop.sh          # formatters, linters and test runners from main see no forks (needs bun and uv)
```
