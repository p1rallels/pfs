---
name: pfs
description: Fan work out to parallel pfs forks (instant APFS clones of a repo, each on its own branch and ports) with headless agents (pi, or any harness that runs one task from the command line), then land it back through the main folder. Covers prepping the repo, splitting and writing sub tasks, the new -q → run --bg chain, waiting, reviewing, the rm → merge fan-in, fork-of-fork, and the etiquette that keeps parallel forks from colliding. Use when the user wants work split across agents or forks, subs/subagents on a repo, "pfs builds X", or to review and land what forks did.
---

# pfs: fan out, fan in

You orchestrate from the **main folder**. Subs work in forks. Work comes back only through `pfs rm` + a merge you do in main.

## The model

- **A fork is a folder** outside the repo, `~/.pfs/forks/<repo>/<n>` (`pfs path <n>` prints it): an APFS clone of the main folder as it is (node_modules, `.env`, uncommitted work and all). Nothing run in the main folder sees forks, so no tool needs a setting.
  - It has its own full copy of `.git` and its own branch `pfs/<n>-<name>`.
  - It gets ports `40000 + 10n …`, delivered as `$PORT`.
  - Nothing walls it. The rules are words: subs follow the repo's AGENTS.md (block below), and nothing is added to a sub's prompt but its task.
- **A sub is a background run:** `pfs run <n> --bg pi -p "<task>"`. pfs doesn't know it's an agent. Any harness that runs one task headless works the same (`claude -p`, `codex exec`).
- **Main is the template.** Work lands in main only through `pfs rm <n>`: it saves the branch into main as `pfs/<n>-<name>`, then deletes the folder.
  - Forks never push. You push from main.
  - rm won't delete a fork while anyone works in it (a process with its folder there, a conversation mid-turn) or while it has uncommitted work.
- **A fork's git settings are its own:** main's are copied at creation, and nothing of a fork's comes back (rm lands only the branch).
- **Forks are flat:** no forks inside forks. A fork *of* a fork is fine (`--of`), and lineage lives in the ledger.

## Commands

```sh
pfs new <name> [-q]                   # fork main as it is; -q prints only the number
pfs new <name> --of <n>               # copy fork n as it is (uncommitted work, caches); n keeps going
pfs new <name> --from <rev>           # clean start at a commit; a live fork's pfs/<n>-x works too
pfs run <n> --bg [-q] <cmd...>        # start something in fork n, in the background; -q prints only <n>.<run>
pfs run <n> <cmd...>                  # same, in the foreground (git log, tests, a commit)
pfs ls [--overlap] [--json]           # work to land (2 commits · uncommitted), what's running or how it ended, lineage
pfs wait [<n>[.<run>]...] [--timeout <secs>]  # block until bg runs end; exit 0 ok, 1 failed/died, 124 timeout
pfs logs <n>[.<run>] [-f]             # a bg run's output (with pi -p: its final answer)
pfs diff <n> [--stat | -- <paths>]    # everything fork n changed since it started, uncommitted included
pfs open <n>                          # the door: your shell in fork n, $PORT set (for a human)
pfs path [<n>]                        # fork n's folder: cd "$(pfs path <n>)"
pfs rm <n>... [--force]               # land each branch in main, delete the folders; one plain merge note each
```

`new` and `rm` run from the main folder. Everything else works from anywhere, into any fork. Outside any repo, `pfs ls` lists every repo's forks and flags the ones whose repo is gone.

## The loop

### 0. Prep main (once per repo)

- **Commit main**, or knowingly keep its wip: forks copy it as-is. A committed main keeps every fork's diff readable, and a sub's `git add -A` can't sweep your wip into its branch. `pfs new` names the uncommitted files a fork carried; if rm's note later says `once X (uncommitted here) is committed or stashed`, that's your wip in the merge's way.
- **AGENTS.md** in the repo: build/lint/test commands, the code style, and the fork etiquette below. It's the only instructions a sub gets besides its task.
- **A formatter config** (e.g. `rustfmt.toml`) so nobody reformats whole files. Without one, one sub's `cargo fmt` conflicts with every other fork.
- **Know what's main's** and keep it for fan-in: shared files (below), anything outside the repo, pushing.

### 1. Split

- **Independent tasks,** one feature each. 2-4 at once is a size you can review properly.
- **Shared files are main's:** docs, changelogs, e2e scripts, task trackers, lockfiles. Subs don't edit them; they say what to add.
- **Two tasks touching the same file is fine when it's natural.** Expect a conflict and merge them one at a time.
- **Don't split what's coupled.** If B needs A's code, do A first or fold B into A.
- **Coupled but big:** while sub A changes what a new piece will plug into, sub B builds that piece as a standalone, unit-tested module nobody calls yet, touching nothing A touches. After both land, a third sub wires it in.

### 2. Write each task (one paragraph)

- **The behavior, exactly:** CLI/API shape, output lines, exit codes, the edge cases you care about.
- **The invariants:** e.g. "uncommitted must mean what rm means: share one function".
- **Known traps:** name clashes ("`WaitArgs` already exists"), perf budgets with numbers ("20 forks under 0.5s"), files another fork is rewriting right now.
- **End with:** "unit-test the pure parts. follow AGENTS.md." The sub commits on its branch.
- **Leave the implementation to the sub.** Its report covers its judgment calls and the e2e checks main should add (AGENTS.md asks for that).

### 3. Launch

One chain per task, from the main folder:

```sh
task='add `pfs wait [<n>...]`: …. unit-test the pure parts. follow AGENTS.md.'
n=$(pfs new wait -q) && r=$(pfs run "$n" --bg -q pi -p "$task")
```

- **Quote the task in single quotes** (or a variable, as above): backticks and `$` inside double quotes run in your shell.
- `-q` prints `5`, then `5.1`. Keep the run numbers: they name the fork too.

### 4. Wait

- `pfs wait <n>.<run>...` blocks and prints one line per run as it settles (`5-wait   bg 5.1 done`). A sub that already ended, even crashed in its first second, still reports how. `pfs wait <n>` waits on whatever runs in the fork now.
- To peek meanwhile: `pfs ls` (running, or how it ended) and `pfs diff <n> --stat`. `pi -p` prints only at the end, so its log is empty until then.
- The conversation so far is the harness's business: pi files sessions by folder, so `cd "$(pfs path <n>)" && lookup last latest` reads it.

### 5. Review

- **The report:** `pfs logs <n>`. With `pi -p` it's the sub's final message: its judgment calls and the e2e checks it lists.
- **The work:** `pfs ls` for `N commits · uncommitted`, then `pfs diff <n> --stat` and `pfs diff <n> -- <paths>`.
- **Its history:** `pfs run <n> git log --oneline main..HEAD` (the fork's `main` is main as it was at fork time).
- **Merge order:** `pfs ls --overlap` lists the files several forks changed.
- **Duplication:** parallel subs reinvent small helpers. Look for it and fold it together when merging.

### 6. Fan in

- **One at a time:** `pfs rm <n>`, read the note, merge, build and test, then the next.
  - The note reads `vs main right now: merges clean` or `2 files would conflict (a, b) · resolve when merging`.
  - Merge with `git merge --no-edit pfs/<n>-<name>`.
  - The note is a snapshot of main *now*. Re-read it after each merge.
- **All at once** (`pfs rm 3 5 7`) is fine when `--overlap` shows they don't share files.
- **"has uncommitted changes":** look with `pfs diff <n>`. Then commit it (`pfs run <n> git commit -am …`) or drop it with `--force`.
  - Main's wip that the fork was born with and never touched doesn't count.
- **"has someone working in it":** a process has its folder there or a pfs run is still going, one line each with what to do (`kill -- -<pid>` for a bg run, "exit that shell" for a door). Wait for it or stop it. `--force` never skips this, and rm never kills anything.
- **"pi conversation … is mid-turn":** a sub or a pi-web chat is still thinking or running tools there. Wait. If it crashed (a turn left open forever), `--force`. A sub you killed doesn't count once its run has ended.
- **After the last merge:**
  1. Full tests, plus the e2e checks the subs listed.
  2. Push from main.
  3. `git branch -d pfs/…` the merged branches.

## Fork of a fork

- **Something interesting in fork 5, as it is right now:** `pfs new try-b --of 5`. It's a copy, uncommitted work included; fork 5 keeps going.
- **A clean start at fork 5's last commit:** `pfs new clean-b --from pfs/5-x`. Works while 5 is open, and after rm.
- **Lineage:** `pfs ls` shows `of 5` / `from pfs/5-x (abc1234)`. Removing 5 doesn't touch its copies.

## A teammate merged upstream

To update one fork: `pfs run <n> git fetch origin`, then `pfs run <n> git rebase origin/main`. Network and credentials work in a fork, and main doesn't move.

## AGENTS.md block for parallel work

```markdown
## working in a pfs fork
- write only inside your fork. reads are open: other forks and the main folder are someone else's work in progress, so read them, never write there. `pfs ls` says which fork you're in (the `*`)
- commit on your branch (`pfs/<n>-…`) when <fmt, lint, tests> are clean. don't switch branches
- finish clean: everything worth keeping is committed before your final message. `pfs rm` lands commits and nothing else
- never delete a fork's folder by hand, yours or anyone's: `pfs rm` from the main folder is the only way out
- don't push: a fork has your credentials and the network, but the main folder reviews, merges and pushes
- servers listen on `$PORT` (or `pfs port`), never a fixed port like 3000: parallel forks would collide
- forks are flat: `pfs new` fails in here. if the task needs more hands, say so in your final message
- <fmt/lint/test commands, e2e script> work in here
- don't edit <docs, e2e scripts, task trackers> unless the task says so: the main folder does those at fan-in. say in your final message what checks to add
```

## Pitfalls we hit

- **Hardcoded totals in e2e scripts** (forks, ledger lines) break as soon as a section is added. Compare before and after instead.
- **Checks that can't fail.** Pair a check with a control proving the bad case happens without the fix (or run the script against the old binary: the new checks should fail there):
  - `gpg-connect-agent --no-autostart` exits 0 even when no agent is running
  - a substring test for "pi" matched "pid"
- **"Forks share main's settings live."** They don't: settings are copied at fork time.
- **e2e scripts that assume where they run.** A check grepping for `.forks` broke once the script ran inside a fork (whose path has `.forks` in it). Match the full path you mean.
- **The merge note goes stale** with each merge in a big fan-in. `--overlap` is the plan; the note is the current reading.
- **Two writers on one conversation can't see each other.** Don't `pi -c` in a door while that fork's sub is still running (the door lists what's running).
