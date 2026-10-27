# pfs

forks of a folder, for you and your agents. `pfs` gives a repo instant, private copies of itself, each on its own branch with its own ports, so the same codebase can be worked on in several places at once, by hand or by subagents, without worktrees, vms or setup scripts.

status: v0.4 (session `01a150cf`): forks live outside the repo, in `<forks dir>/<repo>/<n>` (`~/.pfs/forks` by default, m-dcbb), so nothing run from the main folder walks into them and `git clean` can't reach them or the ledger. `sandbox/interop.sh` reads 0 for every formatter, linter and test runner run from main, with no setting in any of them. `pfs path <n>` prints a fork's folder, and `pfs ls` outside any repo lists every main's forks, flagging the ones whose main is gone. built with pfs: one sub for the layout, one in parallel for the every-repo listing as an unwired module, wired at fan-in. `sandbox/hammer.sh` = 276 checks green. v0.3 (session `01a13e1f`): the walls are gone (m-1a56). pfs is folders, branches, ports and runs, and the rules are words in AGENTS.md. `pfs rm` refuses while anyone works in a fork: a process with its folder there, a harness conversation mid-turn, or uncommitted work (m-9df3, with m-4de7's door hints). built with pfs: two subs in parallel (the teardown; a standalone "who's working here" module), then a third wired it into rm and, with no walls left, ran the end-to-end script inside its own fork. `sandbox/hammer.sh` = 221 checks green. v0.2 (session `01a13d07`): pfs is for every harness. `--agent` is gone: a sub is `n=$(pfs new x -q) && pfs run "$n" --bg pi -p "<task>"`, `pfs ls` says how the last bg run ended, the brief's rules are words in AGENTS.md, and the harness table (`src/harness.rs`) is the only place pfs knows a harness. v0.1 built, hammered and dogfooded (session `01a13c14`): the door, `--of`, `--from` a live fork's branch, rm's merge note, then built with pfs itself: `pfs wait`, `pfs rm` with several forks, `pfs ls` work columns and `--overlap`, per-file "uncommitted". the orchestration loop is a pi skill: `skills/pfs/SKILL.md`. dogfooded on a private github repo (`p1rallels/pfs-demo`) with real pi subs, a fan-in with a real conflict, and a push. v0 (session `01a13b3e`): every v0 command, waiters, the ledger, venv relocation, and the wall (history below). the threat model, signing, and the git design: sessions `01a13b9f`, `01a13baf`. origins (as `git fork`): pi sessions `01a13236`, `01a133b3`. this design: `01a138e1`. use `lookup catchup <id>` for the reasoning behind any decision.

## invariants

what dogfooding checks against. each one has hammer checks.

- **a fork is a folder, and the rules are words.** everything pfs starts in fork n starts in its folder (`pfs path n`) with `$PORT`. nothing stops it writing elsewhere: AGENTS.md asks agents to write only inside their fork, commit on their branch, use `$PORT` and not push, and they do.
- **rm never loses work.** it refuses while anyone works in the fork (a process with its folder there, a pfs run still going, a harness conversation mid-turn) and while there's uncommitted work; `--force` skips only the last two. it never kills anything, and it lands the fork's branch in main as `pfs/<n>-<name>` before deleting the folder. rm is the only way work reaches main; nothing of the fork's settings comes along.
- **forks never push.** etiquette, not enforced: a fork has main's `origin`, your credentials and the network. you push from main.
- **git settings flow one way:** main → a new fork, at creation. a fork may change its own; rm brings back only the branch.
- **folders are flat, lineage is in the ledger.** every fork's start (`base`, `from`, `of`, parent session) and end (`saved_branch`) is recorded and shown by `pfs ls`.
- **pfs owns places, for every harness.** pfs never starts an agent: an agent in a fork is a bg run like any other (`pfs run <n> --bg pi -p "<task>"`, or `claude -p`, `codex exec`, a script). the one place pfs knows a harness is the harness table (`src/harness.rs`): the env var naming its conversation, so `new` records the parent as lineage, and how to read the conversations it files for a folder, so `rm` sees one that's still working. pi is the only entry; others come as PRs.

threat model: honest frontier agents and your own mistakes, not hostile ones. a sub knows it was spawned for one job, in one folder: its folder, its branch and AGENTS.md tell it so. pfs enforces nothing about where a process writes.

## where it fits

small tools, one job each. they join on the folder: a harness files its conversations by working folder (pi does), so a fork's path finds them.

| tool | job |
|---|---|
| `lookup` | reads sessions: what happened |
| `pfs` | makes forks: where work happens |
| `msg` | writes into sessions: agent → agent, you → agent. a session started from another's bash is its child; a child's end reaches its parent as a message |
| `relay` | keeps a session going: nudges at 40% context (then every 5%), resets it in place with a catch-up from lookup |

"orchestration" is a skill that combines them, not another binary: the orchestrator forks with pfs, starts subs from its own bash (lineage), waits on msg (`msg_wait from: "children"`), lands through rm, and relays at checkpoints.

pfs is forks first. the main use is you driving several forks of one repo, one terminal per fork. subagents are just another occupant of a fork.

## why apfs clones

- `clonefile(2)` copies a whole folder copy-on-write in one syscall. cc/pi (22.6k files, 379 MB incl. node_modules) took 0.42s and ~2 MB of extra disk. `cp -c -R` does the same file by file in 6.2s.
- a clone is a full standalone repo: node_modules, `.env`, local data and uncommitted work come along. nothing to install or copy.
- worktrees only get tracked files (reinstall deps, copy `.env`, every time) and share one repo, so a branch can only be checked out once. conductor is a worktree manager with setup scripts.
- vms are slower and heavier: exe vms took ~2–2.5s to be usable, a colima box ~3s to delete. docker / gondolin are too much for this.
- macos only for now (apfs).

## layout

```
~/.pfs/                   $PFS_HOME: pfs's one home
  config.toml              [ports], [forks] dir
  forks/                   the forks dir ([forks] dir moves it)
    pfs-cli/               one folder per main, named after it (pfs-cli-2 on a clash), 0700
      .main                the main's path: whose forks these are
      README.md            a sign: remove with pfs rm from the main, never by hand
      5/                   fork 5: a full apfs clone of the main
        .git/pfs/
          runs.jsonl       what ran in fork 5: starts and exits
          1.log            output of fork 5's first background run
<repo>/.git/pfs/
  forks-dir                this main's forks folder: the record wins over [forks] dir
  ledger.jsonl             every fork that ever existed: created, removed
  ledger.lock
```

- **why outside the repo** (v0.4, m-45bc then m-dcbb): in `<repo>/.forks/`, prettier, eslint, biome, jest, vitest, black and isort run from main formatted, linted and tested every fork, and their `--write`/`--fix` rewrote the forks' work in progress. docker's build context, tar and rsync took forks along, and `git clean -fdx` deleted the ledger (`-ffdx` every fork). no tool-agnostic fix existed: tools share no ignore file (some read `info/exclude`, prettier and black only `.gitignore`, the rest only their own config), and the names everyone skips (`.git`, `node_modules`) are matched on absolute paths by tools inside the fork: jest found no tests in a fork under `.git/`. outside the repo, `sandbox/interop.sh` reads 0 from main for all 18 tools with no setting, and the write modes change nothing in the forks.
- **same disk:** clones only work within one volume, so before a main's first fork pfs checks the repo and the forks dir share one. if not, one error says to set `[forks] dir` to a folder on the repo's disk. no fallback.
- **one folder per main, not one flat folder:** a `..` slip from a fork stays inside its own main's forks. a main is never above a fork: in `.forks/5`, `../..` was the main itself.
- **0700:** pfs makes `$PFS_HOME`, the forks dir and every per-main folder private. forks carry `.env`, and `~` is often 750 while `~/Desktop` is 700.
- **the record wins:** `[forks] dir` only places a main's first forks folder. the main records it in `.git/pfs/forks-dir`, so changing the setting never strands forks.
- **the ledger lives in the main's `.git/pfs/`:** git never tracks `.git`, no tool walks it, `git clean` can't reach it, and it moves with the repo. `new` wipes `.git/pfs` from every clone, so no fork carries it.
- **moved and copied mains:** a moved main signs its folder again (`.main`) on its next pfs command; until then its forks act as mains of their own. a copy never takes over the original's forks: its first `new` gets a folder of its own.
- **orphans:** a deleted main leaves its forks behind. `pfs ls` outside any repo lists every main's forks and flags the ones whose main is gone or replaced, with each fork's work and how to keep or drop it. pfs never deletes them.
- **from 0.3:** the first command in a main moves `.forks/ledger.jsonl` into `.git/pfs/` (ids carry on) and removes the emptied `.forks/`. a main that still has 0.3 forks (`.forks/<n>`) is refused until they're landed with 0.3's `pfs rm`. the config moves by hand, from `~/.config/pfs/config.toml` to `~/.pfs/config.toml`.
- a fork clones each top-level entry except `.forks/`, so a 0.3 leftover never rides along.
- a fork's runs live inside it, in its `.git/pfs/`, so they go with the fork. git never tracks anything in `.git`, so an agent's `git add -A` can't commit logs and no exclude entry is needed (same trick git-lfs uses with `.git/lfs`). `pfs rm` deletes them with the folder.

## forks

- **ids:** sequential per repo, from 1, never reused, unlimited. pi files sessions by folder path, so a reused fork folder would inherit the old fork 5's conversations, and its saved `pfs/5-…` branch would collide.
- **names:** `pfs new add-csv` → fork `8-add-csv`, branch `pfs/8-add-csv`. no name → `pfs/8`. pfs picks the number: a leading `<number>-` in the name is stripped and the final name echoed (`pfs new 7-add-csv` → "created 8-add-csv"). the one casualty is a name like `404-page`. a bare number is an error: `pfs new 5` reads like "a fork of 5" or a slip for `pfs open 5`, and would have made a fork labelled `9-5`. the error points at `--of 5` and `pfs open 5`.
- **refer by number:** `pfs open 8`, `pfs rm 8`. the name is a label.
- **three starts, pick one:**
  - `pfs new` = a copy of the main folder as it is, uncommitted work included. a note names the uncommitted files it carried (one tree diff off the start snapshot pfs takes anyway): a `git add -A` in the fork would commit them on its branch.
  - `pfs new --of 5` = a copy of fork 5's folder as it is: its uncommitted work and build caches, a new branch on top of its commits. fork 5 keeps going, main isn't touched, and no git runs in fork 5 (it's a file copy). this is "fork that": something interesting is happening in fork 5, take it from there.
  - `pfs new --from <rev>` = a clean start at a commit (ignored files like `.env` and `node_modules` come from main). any rev main has (`main~3`, a teammate's `origin/x`, a removed fork's saved `pfs/5-x`), or a live fork's `pfs/5-x`: pfs copies that branch from the fork into the new one, not into main.
- **who's writing in what it copies:** a clone taken while a process writes (a bun dev server's sqlite in WAL mode: the db, its -wal and -shm) can catch a file mid-write. pfs can't make that copy consistent, so it names the writers while redoing is cheap: one `lsof -b -w -d 0-999999 -Fpcfatn +c 0`, side by side with the clone and stopped after 3s, keeps regular files open for writing (`w`, `u`) inside the folder being copied (main, or fork n for `--of`), component-wise on resolved paths. pfs's own `.git/pfs/` doesn't count (every bg run's processes hold its log open through stdout, and the copy drops it), nor does `.forks`. one note per process (`note: bun (pid 4242) has var/v2.sqlite, var/v2.sqlite-shm, var/v2.sqlite-wal open for writing in main, so the fork's copy may be torn. …`), and the fork is made anyway. a check that can't run says so in a note. cost: ~2ms on a 1,000-file repo; in a 1-file repo the clone is faster than lsof's ~80ms, so it shows (~75ms → ~130ms). vim swap files, `target/.cargo-lock` and a git command mid-write are real writers and get named too.
- **fork of a fork:** no nesting, and no rm needed. `--of 5` (as-is) or `--from pfs/5-x` (clean). removing 5 later doesn't touch its copies: each fork has its own git.
- **branch:** each fork checks out its own branch, `pfs/<n>-<name>`, at creation.

## identity comes from the path

no env vars for identity. pfs walks up from the current folder to the nearest repo root (the closest folder with a `.git`):

- **fork id:** if that root is `<folder>/<n>` and `<folder>/.main` names a main whose `.git/pfs/forks-dir` names `<folder>` back, you're in fork n of that main. otherwise it's a main folder
- **ledger:** `<main>/.git/pfs/ledger.jsonl`
- a repo nested inside a fork (a test fixture like `sandbox/work/toy` in a fork of pfs-cli) is a main folder of its own. "outermost `.forks/<n>` in the path" broke exactly that, i.e. pfs building pfs
- a 0.3 fork (`<main>/.forks/<n>`) is refused, so `pfs new` there can't make it a main

the path can't go stale, unlike an inherited env var (a moved main re-signs `.main` on its next command). agents see it too: pi puts the working folder in the system prompt, and pi's footer shows the git branch (`pfs/8-add-csv`) for free.

forks and sessions are different things:

- **fork = the folder.** stable, never reused.
- **session = a conversation that happened in it.** one fork has many sessions over its life. pi makes a new session id on every fresh `pi`, `/fork`, `/clone` and `--fork` (`/tree` stays in the same file). every session header records its working folder, so a session already says which fork it belongs to.
- **`pfs rm` deletes the folder, never the sessions.** those live in `~/.pi/agent/sessions/<folder>/`, so lookup still reads them afterwards.

## ports

fork n owns 10 ports:

```
base(n) = 40000 + (n mod 900) × 10        fork 5 → 40050–40059
```

- **no state:** a pure function of the fork number. never stored, never "skip if busy" (that would need remembering).
- **why there:** below macos's ephemeral range (49152–65535), away from 3000 (every dev server's default) and 5000/7000 (taken by ControlCenter, aka airplay receiver). `n × 1000` breaks on those and overflows at fork 66.
- **why not hashed or random:** birthday collisions. 20 forks in 1000 slots = 17% chance of a clash. sequential can't clash until the wrap.
- **wrap:** every 900 forks (fork 905 reuses fork 5's block). only a clash if both are alive at once.
- **delivery:** `$PORT` is the one env var. pfs sets it on everything it starts in a fork (the door, `run`), since most servers read it by default. it delivers the port, it isn't identity.
- **scripts:** `pfs port` prints the current fork's base port: `PORT=$(pfs port) bun dev`.
- agents never need to probe ports or `kill $(lsof -ti:3000)`: their block is theirs.

## words, not walls

until v0.3, every process pfs started in a fork ran inside a macos seatbelt profile that let it write only to its fork plus an allow-list (history below). it went in session `01a13d07`:

- **agents follow words.** AGENTS.md's "write only inside your fork, commit on your branch, `$PORT`, don't push" held in every dogfood run.
- **a wall only ever existed on processes pfs starts:** seatbelt applies at process start. pi-web hosts every conversation in one long-lived process started elsewhere, so a pi-web chat working in a fork was never walled, and couldn't be. the same goes for any in-process harness.
- **nothing core needed it.** forks, branches, `$PORT`, the ledger, runs and their waiters, `new`/`ls`/`wait`/`diff`/`run`/`open`/`rm`, `--of`/`--from`, overlap and rm's merge note all stand on their own.
- **and it cost:** an allow-list per toolchain and per harness with a command to manage it, a git settings lock, pfs's own git wrapped in walls, gpg-agent warm-ups, a shell-history exception for the door, and "walls can't stack": no end-to-end script inside a fork, nothing from inside one fork into another.

what's left are the places pfs owns: a fork is a folder (`<forks dir>/<repo>/<n>`) with a branch, a port block and a record of what ran there. from inside a fork, `pfs run`, `pfs diff` and `pfs open` into another fork just work, `pfs ls` checks every fork's work, and `sandbox/hammer.sh` runs anywhere. old data still reads: the ledger's `allow` lines are skipped, `[wall]` tables in `~/.config/pfs/config.toml` are ignored, and a repo's `.pfs.toml` isn't read at all.

### git in a fork

a fork's `.git` is a full copy of main's (history, branches, settings, hooks), not a checkout of main's git. pfs makes `pfs/<n>-<name>` in it and switches to it, so each fork is its own repo that starts out identical. agent commits go into the fork's copy; main sees nothing until `pfs rm`.

- **pfs runs plain git in the fork's folder** (`new`'s setup, `diff`, `ls`, `rm`'s checks), so the fork's settings apply: its hooks, filters and pager run, as you.
- **a fork's settings are its own.** `git config`, hook installs and `git remote add` work in a fork and stay there: settings flow main → a new fork at creation, and `rm` brings back only the branch (`git fetch` from the fork into main).
- **new branches track nothing** (`--no-track`): a fork's work lands through `rm`, not a pull or a push.

## the ledger

one line per thing that happened, only ever appended, never erased. same idea as a pi session file. two files:

**`<main>/.git/pfs/ledger.jsonl`, the forks.** written by `new` and `rm`, which run from the main folder.

```jsonl
{"type":"fork","id":8,"name":"add-csv","branch":"pfs/8-add-csv","base":"a1b2c3d","parent_session":"01a138e1…","ts":"…"}
{"type":"fork","id":9,"name":"try-b","branch":"pfs/9-try-b","base":"e4f5a6b","of":8,"ts":"…"}
{"type":"rm","id":8,"saved_branch":"pfs/8-add-csv","ts":"…"}
```

**`<fork 8>/.git/pfs/runs.jsonl`, the runs inside one fork.** written by the waiter (or by pfs itself for a foreground run), whether the run was started from the main folder or from inside the fork.

```jsonl
{"type":"start","run":1,"kind":"bg","cmd":"pi -p fix the csv export","pid":4242,"ts":"…"}
{"type":"exit","run":1,"code":0,"ts":"…"}
```

- **forks:** what each fork is (name, branch, base, `from`, `of`), who started it (pi exports `PI_SESSION_ID` to bash, so the parent session is recorded for free), its removal.
- **runs:** starts and exits (kind, pid, exit code, when). they die with the fork. rm reads the exits' times to tell a dead run's conversation from a live one.
- **old lines still read:** `--agent`'s `task`/`session` fields are ignored, a run of kind `agent` reads as `bg`, and v0–v0.2's `allow` lines are skipped.
- **in neither:** code (git), conversations (the harness's), ports (computed).
- `pfs ls` reads the ledger top to bottom for the active forks, then each fork's `runs.jsonl` for what's running.
- **concurrency:** every append, and every id allocation, happens under a file lock, so runs finishing at once or two `pfs new` at once can't garble anything or share a number.
- **trust:** anything running in a fork can write its run records. that's bookkeeping, and the threat model is honest agents.
- **size:** a few hundred bytes per event. never needs erasing.

## running things in a fork

everything pfs starts in a fork gets the fork as its folder, `$PORT`, and a line in the fork's run records.

### the door

`pfs open 5` is `cd "$(pfs path 5)"` plus `$PORT`: your `$SHELL` in the fork, one line saying where you are (`fork 5-csv · branch pfs/5-csv · ports 40050-40059 · exit to leave`). `exit` brings you back. what you run in there is yours: `pi`, `pi -c`, `pi -r`, an editor. a fork is a folder, and the conversations in it are the harness's: pfs doesn't pick one for you. `pfs new --open` makes a fork and walks you in.

- **something still running:** the door lists what pfs has running in the fork (from its run records: `5.1 bg (pid 4242)`), so you don't walk in on an agent's work unawares. nothing about conversations: the door knows nothing about pi, by design.
- **recorded:** the door is an `open` run, so `pfs ls` shows it (`open 5m`) and `pfs rm` won't pull the folder out from under it: it says "exit that shell", or "that's the door you're in" when rm is typed inside it.
- **nothing added:** a pi you start in the door is a plain pi, like a sub's: no fork line, no extra prompt. it sees the folder (`~/.pfs/forks/<repo>/5`) and the branch in its footer like anywhere else, and AGENTS.md says the rest.
- **cd vs the door:** `cd "$(pfs path 5)"` works too. the door adds `$PORT` and the record.

- **foreground (default):** `pfs run 3 cargo test`. output goes straight to your terminal, and pfs exits with the command's exit code. it's `cd "$(pfs path 3)" && cargo test`, minus the cd, plus `$PORT` and the record.
- **background:** `pfs run 5 --bg bun dev`. returns at once. each run gets a number in its fork (`5.1`, `5.2`), so several runs per fork never mix. output → `<fork 5>/.git/pfs/1.log`. no cap, no trimming. deleted with the fork.
- **from inside a fork, the number is optional:** `pfs run --bg bun dev`. the path says which fork. any other fork's number works too.

### the waiter

background runs (agents included) get a tiny waiter process. macos hands a finished program's exit code only to its parent, so something has to be waiting to write "exited 1 at 14:32" down. roughly:

```sh
# in fork 5's folder
sh -c 'bun dev > .git/pfs/1.log 2>&1; echo "{\"type\":\"exit\",\"run\":1,\"code\":$?}" >> .git/pfs/runs.jsonl'
```

- pfs starts it in its own process group (`setsid`). pi's bash tool kills a command's whole process group on timeout or esc, terminals send their group a hangup on close, and some harnesses kill leftovers when a command ends. the waiter is out of reach of all three, so `pfs new` / `pfs run --bg` return in well under a second and the work survives pi quitting.
- no tmux. tmux was only ever the babysitter.
- foreground runs don't need one: pfs itself waits.
- the waiter only writes inside the fork (`.git/pfs/`), so it works the same whether it's started from the main folder or from inside the fork.
- the `start` line records the command's pid, not the waiter's. killing that pid ends the run, and the waiter still writes its exit line.
- status commands (`pfs ls`) only read. the waiter is the one writer that has to be there when things end.

## agents (a bg run in a fork)

```sh
n=$(pfs new add-csv -q) && pfs run "$n" --bg pi -p "add a csv export for balances"
```

- **an agent is a background run like any other:** `$PORT`, a waiter, a run number, an exit code. pfs doesn't know it's an agent, which is the point: pi, `claude -p`, `codex exec` and a shell script all start the same way.
- **chaining:** `pfs new -q` prints only the fork number and `pfs run --bg -q` only `<n>.<run>`, so scripts and agents chain them without parsing. the human lines' notes go to stderr.
- **following it:** `pfs ls` (running, then how the last bg run ended: `bg 5.1 done 3m ago`, `failed (exit 1)`, `died, started 10m ago`), `pfs wait` (blocks until it ends), `pfs logs <n>` (what it printed: with `pi -p`, its final answer).
- **the conversation:** the harness's own reader, outside pfs. pi files sessions by folder, so `lookup` finds a fork's by its path.
- **the rules are words:** a sub reads the repo's AGENTS.md (every harness does): write only here, commit on your branch, don't push, `$PORT`. the rest it finds itself: its folder and its branch.
- **why not `--agent`:** v0.1 had `pfs new <name> --agent "<task>"`: a pi session id picked up front, a brief appended to pi's system prompt, `pfs logs` handing off to lookup. all pi-specific, and pfs is for every harness, so it went (session `01a13c14`). the brief's rules moved to AGENTS.md.
- **adding a harness:** one entry in the harness table: the env var naming its conversation, and a reader for the conversations it files for a folder (which ones, their last write, whether a turn is open). no harness code anywhere else.
- **vs pi's subagent example:** those are children on a leash: same folder, `--no-session` (no trace, lookup can't see them), esc kills them. fine for quick scouts. pfs is for long jobs that edit code.

### forks are flat

- no forks inside forks. every fork lives in its main's forks folder. need more hands? make more forks from the main folder. a fork *of* a fork is fine (`--of 5`): flat folder, lineage in the ledger.
- `pfs new` inside a fork fails: "forks are flat, create forks from the main folder". a sub that needs another fork says so in its final message (or asks its parent via `msg`). it's layout, not security.

## commands

```
pfs new [name] [--from <rev> | --of <n>] [--open | -q]
pfs ls [--overlap] [--json] [--all]  active forks, * marks the one you're in, lineage, work to land
                               (outside any repo: every main's forks, and the ones whose main is gone)
pfs port                       the current fork's base port
pfs path [<n>]                 a fork's folder, absolute: cd "$(pfs path 5)"
pfs wait [<n>[.<run>]...] [--timeout]  block until those forks' bg runs (or those runs) have ended
pfs open <n>                   the door: your shell in fork n, $PORT set. exit to leave
pfs run [<n>] [--bg [-q]] <cmd...>  run a command in fork n
pfs logs <n>[.<run>] [-f]      a background run's output
pfs diff <n>                   what fork n changed since it started, uncommitted included
pfs kill [<n>]                 stop what's left working in fork n: never its pfs runs, never the caller
pfs rm <n>... [--force]        save each branch to main, delete the forks
```

`new` and `rm` write the ledger, so they run from the main folder (`new` because forks are flat, `rm` because it lands branches in main). everything else works from anywhere, into any fork. inside a fork, the path fills in `<n>`. in the main folder, a command that targets a fork needs the number and errors without it: there's no fork to infer, and plain commands in main don't need pfs.

### pfs ls

```
$ pfs ls
  3-cli-check      40030-40039   2 commits
  5-csv-addition   40050-40059   1 commit · uncommitted · bg 5.1 running 7m
  6-csv-b          40060-40069   of 5 · 1 commit · bg 6.1 done 2m ago
  7-search-query   40070-40079   bg 7.1 running 1h · log 1.2G
  8-csv-c          40080-40089   of 5 · uncommitted · bg 8.1 died, started 10m ago
```

active forks only (the folder exists). per fork: lineage (`of 5`, `from HEAD~1 (4e7547f)`), work to land (commits since its base, and uncommitted work in rm's sense), what's running, how its last bg run ended once none is, and logs over ~1 GB, so nothing quietly eats the disk. `--json` has `commits` and `uncommitted`.

- **the work columns cost git:** 4 git calls per fork (building the folder's tree takes 2), run for all forks in parallel: 20 forks in 0.2s. they write a scratch index and git objects inside each fork, like `diff` and `rm`; the ledger and run records are only read. from inside a fork, every fork is checked the same way; `?` means git itself couldn't tell.
- **`pfs ls --overlap`:** after the table, every file two or more forks changed since they started (what `pfs diff <n> --name-only` shows), with those forks: `src/cli.rs   2-wait, 3-rm-many`. for picking a merge order before a fan-in, so it's just a list, no warnings. one extra git call per fork. main's wip that a fork carries and never touched isn't listed. `--json --overlap` gives each fork `"overlap": {"<file>": [<other ids>]}`.

### pfs wait

`pfs wait 2 3 4` blocks until those forks have no bg run going (no numbers: every active fork from the main folder, or the fork you're in). fg runs and doors don't count: someone sits at those. one line per fork as it settles (`3-rm-many   bg 3.1 done`), exit 0 when everything ended with 0, 1 if any failed or died, 124 on `--timeout` (it says what's still running). it reads only pfs's run records, and skips the run it's inside, so an agent calling `pfs wait` in its own fork doesn't wait on itself.

`pfs wait 5.1` (what `run --bg -q` prints) waits on that bg run only and says how it ended even if it already had, so `pfs wait "$r" && pfs logs "$r"` can't pass a sub that crashed in its first second. a missing or fg run is an error, a fork named whole covers its runs named with it, and waiting on the run you're inside is refused (it can't end first). a fork with nothing running says how its latest bg run ended (`nothing was running · bg 5.1 done 44s ago`), but only runs wait watched count toward the exit code. a death behind either gets the same second look.

### pfs rm never loses work

each fork's git lives inside its folder, so deleting the folder would delete its commits. `pfs rm 5`:

1. **refuses while anyone works in the fork** (see "who's working in a fork"): a process with its working folder there, or a pfs run still going, wherever its folder is now. each occupant is one line, its pid and what to do, with a run's own processes (its waiter, its children) folded into it:
   ```
   $ pfs rm 1
   pfs: fork 1-demo has someone working in it:
     1.1 bg pid 44143  sleep 20 & sleep 20; wait  (stop: kill -- -44143)
     pid 44145  sleep  (stop: pfs kill 1)
   rm never stops anyone, and --force doesn't skip this: rm again once they're done
   fork 1-demo: pi conversation 01a14f00 is mid-turn (running tools, last write 0s ago). wait for it, or if it crashed: pfs rm 1 --force
   fork 1-demo has uncommitted changes. commit them on pfs/1-demo (see them with pfs diff 1), or pass --force to drop them
   ```
   a bg run leads its own process group, so `kill -- -<pid>`. a door is someone's shell: "exit that shell", never a kill, and "that's the door you're in: exit it, then rm from a normal shell" when rm was typed inside it (the door is in rm's own lineage). anything else rm runs under gets "leave it, then rm from outside the fork". a fg run gets "finish or quit it", and every process that's no run's (a server a sub left, a terminal pi, an editor, a shell that cd'ed in) "stop: pfs kill <n>". `--force` never skips this, and rm never kills anything.
2. **refuses while a conversation in the fork is mid-turn,** unless `--force`: a crash leaves a turn open forever, and `--force` is the way out for it.
3. **refuses uncommitted work,** unless `--force`. uncommitted is per file: a file that matches neither the last commit nor how the fork started. so the main folder's wip a fork was born with (an untracked lockfile, an edit) doesn't block rm while the fork leaves it alone, and doesn't ride along into the branch either.

   all three are said at once, so one look is enough.
4. copies the fork's branch into the main repo (`pfs/5-csv-addition`), ready to review or merge. `git branch --list 'pfs/*'` doubles as history, and cleanup is plain `git branch -d`.
5. deletes fork 5's folder (its runs and logs go with it), then appends an `rm` line to the ledger.
6. prints one plain line on how the branch meets main's current branch right now (`git merge-tree`, nothing touched): `vs main right now: merges clean`, `already in it`, or `2 files would conflict (src/text.ts, src/text.test.ts) · resolve when merging`. same exit code either way, no "fix this": it's for picking a merge order, and it goes stale as soon as something else merges. dogfooded: in a 3-way fan-in, merged one at a time, it named the two files that then conflicted. merge-tree only sees commits, so the main folder's own uncommitted files that the merge would write are named too (`merges clean, once README.md (uncommitted here) is committed or stashed`): git refuses the merge until then.

several at once: `pfs rm 3 5 7` takes one look at who's working (one lsof, before the first removal; if lsof can't run, nothing is removed), then checks and removes each fork on its own, in order. a refused one keeps only itself, and rm exits 1 after trying them all. 0.2s for a refusal like the one above.

the fork's sessions stay in `~/.pi/agent/sessions/`.

### pfs kill

why: in a real fan-out a sub stopped its own bun dev server with `pkill -INT -f "bun src/server.ts"`. main and every fork run that exact command line, so it also stopped main's preview server and another fork's dev server mid-task. pfs keeps folders and ports apart, not processes, and the safe moves (`kill <pid>`, `lsof -ti tcp:$PORT`) take thought, so subs reach for a pattern.

`pfs kill [<n>]` stops what rm would list as no run's: each process working in the fork that's no pfs run's, and what it started there (a TERM to `sh -c 'bun …'` leaves bun running, so it asks `present` again without the ones found until nothing new turns up, at most 64 rounds). never a pfs run (bg, fg, a door) or what runs under it, never the caller's lineage (a sub's `pfs kill` runs under its own pi, bash and waiter in that fork), never another folder's process, whatever its command line. SIGTERM to all at once, then up to 3s; a pid is signalled and counted only while its start time matches the listing (a recycled pid isn't the process listed, and a zombie reads as gone). exit 0 once they're all gone, 1 while one is left, with `kill -9 <pid>` next to it; the runs it left alone are listed with rm's hints. ~0.1s.

by folder, not by port: a server in main on a hand-picked port can sit in a fork's block (in a real fan-out, a preview server on 40980 sat in fork 98's), and the folder is what rm goes by, so `pfs kill 5` is exactly what clears rm's "has someone working in it". the cost: a shell someone cd'ed into the fork and an editor's helpers count as leftovers too. interactive shells ignore TERM, so they're reported, not stopped.

## who's working in a fork

rm's first two checks, made of things pfs doesn't own: who has a process in the fork, and whether a harness conversation there is still working. `src/occupants.rs` puts them together; the harness part comes from the harness table.

- **processes:** one `lsof -a -d cwd -Fpcn +c 0` lists every process's working folder (~0.1s for ~430 processes; no tree walk, so node_modules doesn't matter), kept when it's the fork or inside it, component-wise (`…/5` holds `…/5/src`, not `…/50`). it doesn't care who started a process: a sub, a door, a terminal pi or claude, `bun dev`, an editor, a shell that cd'ed in, a pi-web bash command that happens to be running. pfs's own runs still going count too, wherever their folder is now (a door whose shell cd'ed out is still a door). processes that are a run's (under it in the process tree, or its waiter) fold into the run's line, and a loose process's children into it, so a pi sub is one line, not ten. the waiter stands in its run's folder, so a `pfs run 3 --bg` typed from fork 5 leaves nothing in fork 5.
- **conversations:** what lsof can't see is a conversation that's thinking. pi-web hosts every session in one process whose folder is elsewhere, and a model call is network, not a process. but harnesses file their conversations, and pi's say where a turn stands: pi appends a whole entry when each message ends (`appendFileSync` at message_end, never token by token), so the last message is the turn's state. your prompt or a tool's result = the model is about to answer (thinking); an assistant message that called tools = tools running; any other ending (stop, length, aborted, error) = done; other roles (system, `!` commands, extension messages) and other entries are skipped. checked on 400 real session files: the only open one was the live session doing the check. pi files a folder's conversations in `<agent dir>/sessions/--<path>--/` (pi-web too), or flat in `PI_CODING_AGENT_SESSION_DIR`; each file's first line names its folder, so a conversation started in a subfolder of the fork counts, and the lossy folder names (`5/x` and `5-x` encode the same) never decide. pfs reads files and couples to nobody: the reader is the pi entry's `conversations` in the harness table, ~1ms a fork.
- **killed runs:** a `pi -p` sub killed mid-tool (SIGTERM) exits 143 and writes no closing message: its last entry is the killed command's result, so it reads mid-turn forever. so an open conversation doesn't count when nobody is in the fork (no process, no pfs run going) and its last write came at or before the fork's latest recorded run end (+1s: the exit line is ms-precise and written just after the process ended). a died run (no exit line) records no end, so it clears nothing. stays harness-neutral: it compares pfs's own run records with a file's last write.
- **accepted gaps** (session `01a13d07`): a power loss mid-turn and a pi killed in a terminal leave a turn open (`--force`); an error pi auto-retries reads closed for a few seconds; a pi-web chat that's open but idle reads closed (nobody's working); a pi-web chat thinking in a fork where an unrelated run ended later reads as that dead run's. not followed: a `sessionDir` in pi's settings.json, `--session-dir`, and conversations filed under a symlinked alias of the fork's path (`/tmp/…` for `/private/tmp/…`).

## later

- `pfs wait` could use rm's "who's working here" check (a door or a terminal pi, not just bg runs).
- `--context`: start a sub from a copy of the parent's conversation. with `--agent` gone it's harness flags in the chain, no pfs code: `pfs run "$n" --bg pi --fork "$PI_SESSION_FILE" -p "<task>"` (`SessionManager.forkFrom(source, cwd, …)`; `--fork` can't be combined with `--session`, `--continue`, `--resume` or `--no-session`). untested; the open part is role confusion (the sub inherits the parent's "fan out, merge, push").
- a quick throwaway mode: fork, run, show the result + diff, delete the folder, keep one ledger line.
- a footer extension: `ctx.ui.setStatus` → "fork 3 · cli-check" inside a fork, "pfs: 2 running" in the parent.
- deferred from the `git fork` design: `land` (rebase → checks → fast-forward) and `heat` (bounces per file, spine meter).

## pfs builds pfs (v0.4)

forks moved out of the repo (m-dcbb). the layout is one coupled change (place.rs, new.rs and the hammer's 93 `.forks` paths), so it went to one sub (A). in parallel a second (B) built the every-repo listing as a standalone module nobody called yet, against a contract both tasks spelled out (`.main`, `.git/pfs/forks-dir`, `<n>/`, `.rm-<n>/`). the installed 0.3 pfs ran the whole round: it made both forks, ran both subs and landed them. the new build only ever ran in throwaway repos with `PFS_HOME` on scratch: run in pfs-cli's main, it would have moved the ledger out from under the installed pfs mid-round. A took ~27 minutes (+1273/-283, the hammer at 265), B ~9 (one 787-line file).

- **fan-in:** `--overlap` listed only main.rs (two `mod` lines, separate hunks), so both merged clean. main then wired B into `pfs ls` outside any repo (`place::try_here`) and added 11 hammer checks: 151 → 161 unit tests, 265 → 276 hammer checks.
- **A's calls:** `Repo` resolves its main's forks folder once, from the record, never by guessing `<forks dir>/<name>` (that may be another main's). folders are made under the ledger lock with an exclusive mkdir, the record is written last, and `.main` and the record are written whole. the same-disk check runs before the first fork, so a record never points at another disk; EXDEV was tried on a throwaway APFS disk image. 0.3 forks are refused from inside too: otherwise `pfs new` there would make one a main.
- **B's calls:** an orphan's commits are counted against every other branch, local and remote-tracking (the ledger's base lives in the main that's gone), so a pushed branch counts as kept. a folder with no `.git` gets no git call: git would climb to a repo above it, maybe a dotfiles repo in `~`.
- **interop.sh:** from main, 0 for all 18 tools. inside a fork, every tool still sees the fork's own files. the first write-mode row first read 1: caches the in-fork column left in fork 1 (pytest, mypy), not prettier. a reset before the write modes fixed the measurement.

## pfs builds pfs (v0.3)

the teardown (m-1a56) touched almost every file, and rm's check (m-9df3) plugs into what it rewrote, so: one sub for the teardown (A); in parallel, one building the check as a standalone module nobody called yet (B), told to touch nothing A touched: one new file, its `mod` line with `#[allow(dead_code)]`, pi's reader kept in its own `mod pi` because A was rewriting harness.rs; then, after both landed, a third (C) wiring it into rm and the harness table, with m-4de7's door hints. A and B ran 12 minutes each (81 and 42 tool calls), C 17 (65). the orchestration used the installed (walled) pfs until the very end, so all three subs were walled while A and C wrote code without walls.

- **fan-in:** `--overlap` listed only main.rs (A removed two `mod` lines, B added one elsewhere): both merged clean, then C clean. 70 → 95 → 110 unit tests.
- **A's findings:** the hammer as-is collapsed: "editing pfs's config: denied" really wrote to the config once nothing denied it, and every later call failed on bad toml. a check that tests a guard does the damage once the guard is gone.
- **B's findings:** the real sessions folder has both `--tmp-pi-web…` and `--private-tmp-pi-web…`: pi-web files under the path it was handed, a terminal pi under the resolved one, so B matched both spellings (C then narrowed rm to the resolved one: a gap, above).
- **C's calls:** reshaped B's one-call API so a fork whose records can't be read blocks only itself and `--force` never reads conversations; folded a run's processes into one line; moved the waiter into its run's folder (a `pfs run 3 --bg` typed from fork 5 left a stray `pfs` blocking `rm 5`). with no walls in the code it ran the hammer inside its own fork: 178 of 182, the 4 misses being the `ps` checks the old installed pfs's wall still blocked.
- **main's findings:** a hammer check grepping a venv for `.forks` broke once the script ran inside a fork (whose path has `.forks` in it). a control run of the hammer against the old binary showed every new rm check failing there, except one that passed only because the old rm had already removed the fork: paired with a fork-still-there check.

## pfs builds pfs (v0.1)

3 pi subs on pfs itself, in parallel: `pfs wait`, `pfs rm` with several forks, `pfs ls` work columns. each in 4-7 minutes, committed with fmt/clippy/tests clean, each ended with the end-to-end checks it couldn't run (the hammer couldn't run inside a wall back then). fan-in one at a time: the merge note said clean, then "1 file would conflict (src/cmd/rm.rs)" for the second (both touched rm.rs), then clean. the conflict was the import block.

what the subs found or decided on their own: `pfs wait` skips the run it's inside (an agent waiting in its own fork would wait forever); a run with no exit line counts as died only if a second look agrees (the waiter writes the exit just after the process ends); the wall's probe folder collided between parallel threads. the new `ls` work columns then exposed a v0 rm rule: a fork from a dirty main that committed only its own work was refused as "uncommitted". fixed (per-file rule above).

what we fixed so subs could work: rustfmt config (the code wasn't rustfmt-formatted, so one `cargo fmt` would have conflicted with everyone), AGENTS.md (commands, style, parallel etiquette: no docs/hammer/.marbles in sub tasks), `new`/`diff`/`rm` on a test repo inside a wall.

round two used round one's features to run itself: two subs (`ls --overlap`; the brief's lineage line plus a clearer refusal when pfs's own git couldn't reach another fork), `pfs wait 5 6` blocked 334s and exited 0, `pfs rm 5 6` landed both, both merged clean though both touched forks.rs. the polish sub saw (reads are open) that fork 5 was editing ls.rs and stayed out of it, which left the "from HEAD~1 (4e7547f)" wording in two places; folded into one `ledger::Fork::started_from` at fan-in. lesson: parallel subs duplicate small helpers, so look for that when merging.

## dogfood findings (v0.1)

on `p1rallels/pfs-demo` (private, bun + ts), with real pi subs:

- **fan-out:** 3 subs in parallel (slugify, truncate, /health), each done and committed in under a minute, 3-7 tool calls, no denials, nothing left uncommitted.
- **fan-in:** rm → merge, one at a time. the merge note said "merges clean" for the first, then "2 files would conflict" for the second (both appended to `src/text.ts`), which is what happened. resolved in main, 16 tests green, pushed from main.
- **a teammate's merge:** a commit pushed to origin from outside, then `git fetch origin && git rebase origin/main` inside a fork worked (gh's credential helper is fine there). main didn't move.
- **fork of a fork:** `--of 3` carried fork 3's uncommitted NOTES.md, node_modules and `.env`; `--from pfs/3-health` (fork 3 still open) started clean at its commit; main got no branch from either.
- **the door in a real pty with zsh:** right folder, branch and `$PORT` (and, back then, the wall's history and settings-lock findings).
- **the lock vs a sub (v0.1):** asked to install a pre-commit hook, a real sub put it in a tracked `.githooks/`, committed with `git -c core.hooksPath=.githooks`, and said turning it on is main's call. no flailing.
- **the door in your own terminal** (v0.2, m-8aae): `pi` in the door knew it was in fork 14 from its folder and AGENTS.md alone, saved its session under the fork's folder, and `pi -c` continued it.

## dogfood findings (v0.2, the chain)

on `pfs-demo` again, after `--agent` went, with the skill's chain and nothing but AGENTS.md for rules:

- **launch:** two `new -q` → `run --bg -q pi -p` chains in 1s. `pfs wait 11 12` exited 0 about a minute later, and `pfs ls` kept `bg 11.1 done 3s ago` on the board.
- **words held:** told only to "check it for real", a sub ran `bun dev` in the background on `$PORT` (40120), curled it, killed only the pid it started, didn't push, and ended with what to add to the README. the other reused a helper and said so.
- **reports:** `pfs logs <n>` is the sub's final answer with `pi -p`. pi's own stderr warnings (model patterns from your settings) land at the top of the log.
- **peeking:** `cd .forks/11 && lookup last latest` reads a running sub's conversation by folder. nothing in pfs needed.
- **old data:** a fork whose run was started by `--agent` showed `bg 8.1 done 2h ago`; the parent session went into the ledger from `PI_SESSION_ID` through the harness table.
- **fan-in:** `--overlap` empty, `pfs rm 11 12`, both merged clean, 31 tests green, pushed from main.

## dogfood findings (v0.3)

solo, on codemapcli-rs and pfs-demo, after the walls went:

- **cm (m-0673):** `pfs new` in codemapcli-rs (0.23s, with 5 GB of gitignored data along) was the repro: its exclude entry plus a full clone with its own `.git` doubled cm's index (34 → 68 files). the fix was made in that fork and checked there: the installed cm and the fork's build side by side against the same main, the fix flipped off for a control run without main seeing it. then `pfs diff` → `pfs rm` ("merges clean") → fast-forward.
- **interop (m-45bc):** `sandbox/interop.sh`, above. from main, `prettier --write .` changed 8 files across two forks, `biome check --write .` 10, `eslint --fix .` 2, `black .` and `isort .` 2 each. with each tool's ignore setting, tools run inside a fork still see all of it.
- **forks under `.git/`** (the runs trick) would hide them from every tool in main, but jest drops any path with `/.git/` in it: run inside such a fork, it finds no tests.
- **words drift:** pfs-demo's AGENTS.md still said "you're walled" and "git settings are frozen in a fork" a release after the walls went (m-96a2). nothing checks words, so a sub there would have worked around a wall that's gone.
- **fan-out, words only:** two pi subs on pfs-demo (kebabCase; `GET /words`, "check it for real"), ~40s and ~60s, one commit each. no README edits, fan-in lines suggested instead. the route sub took its port from `pfs port`, curled it and stopped its server (rm found nobody working), and read fork 19 to warn of a possible conflict in server.ts. there wasn't one: `--overlap` empty, both merged clean, 41 tests.
- **`--overlap` with nothing shared** printed nothing, same as plain `ls` (m-3f53): now one line says so.
- **`pfs wait` after the fact:** `pfs wait 19 20`, called once both subs had finished, said "nothing was running" twice and exited 0, as it would have for two crashes. a pi sub built `pfs wait <n>.<run>` in fork 13 (5m48s; ran the hammer in its fork, 206/206), with two calls of its own: waiting on the run you're inside is refused, and the look at startup isn't a death's first look (an instant `true` read as died otherwise). 9 hammer checks at fan-in, 5 of them red on the old binary.
- **a fork carries main's `target/`:** warm incremental builds, but `./target/debug/pfs` in a fork is main's binary until you `cargo build` there (`cargo test` and clippy don't make it). trying "the fork's build" first tried main's.
- **the door without a tty:** `printf 'echo $PORT; exit 3\n' | pfs open 16` ran the lines in the fork with `$PORT` set and passed exit 3 through.
- **peeking:** `cd .forks/13 && lookup last latest` showed a running sub reading the hammer, mid-turn.
- **main's wip, swept:** logged a marble in main (uncommitted), forked, and the fork's `git add -A` committed it on the fork's branch: the prep rule in the skill, broken by its author. rm said "merges clean" and `git merge` refused (local changes would be overwritten), because the note read main's commits, not its folder. now it names them (m-d9f3); in the hammer's toy that's 3 files, an untracked `bun.lock` among them. and `pfs new` names main's uncommitted files when it forks, while it's cheap to act on.
- **`ls --all` remembered branches that were gone:** in pfs-demo, 11 removed forks said `saved pfs/1-slugify` and so on, all merged and `git branch -d`'d since, as the skill says. now `saved pfs/1-slugify, deleted since` (one `for-each-ref`, only with `--all`).

## findings (v0)

- **speed:** `pfs new` = 0.40s on a copy of pi-web (26k files, 263 MB, dirty main), vs 3.6s for `cp -c -R`. 20 parallel `pfs new` = 0.46s total, unique ids, ledger intact. first `git status` in a fork is instant.
- **venvs:** the clone's `bin/pip` and `activate` pointed at main's venv, and pip from a fork really did install into main's venv. `pfs new` rewrites venv paths to the fork: `bin/*`, `pyvenv.cfg`, editable `.pth`/`__editable__`/`.egg-link`, whole-path matches only, a few ms. venvs found 1-2 levels down (`pyvenv.cfg`).
- **killing:** a bg command leads its own process group (`kill -- -<pid>` stops it and its children; the waiter still writes the exit). `kill -9` on the waiter: the command lives on, ends as `died`. killing the caller's group (pi's esc/timeout) spares the run.

## history: the wall (v0–v0.2)

v0 to v0.2 put every process pfs started in a fork inside a macos seatbelt profile (`sandbox-exec`): reads open, writes only to the fork plus a layered allow-list. it went in v0.3 ("words, not walls"). what it taught, for the record:

- **seatbelt works for this:** `(deny file-write*)` then `(allow file-write* (subpath <fork>))`, ~0 overhead, grandchildren covered, network untouched. paths must be fully resolved (`/tmp` is `/private/tmp`).
- **but every tool writes somewhere:** toolchain caches (bun, uv, pip, go, cargo, each with its env overrides, probed by a `toolchains.sh` from cold caches), a harness's state (pi: its sessions, `auth.json` and settings lock folders, never `settings.json`, which decides what loads), the door's shell history, mkdir-only parents for a cache's first use. each was an allow-list entry, layered built-in / harness / door / global / repo / fork, managed with `pfs allow`.
- **git needed two guards:** a fork's settings name programs (hooks, filters, the pager, `upload-pack` on fetch), so pfs ran its own git in a fork inside the fork's wall; and a settings lock kept a fork from editing its own `.git/config` and hooks, which meant `git -c` for one-offs and `branch.autoSetupMerge=false` for everything started there.
- **signing:** walled gpg could use a running gpg-agent but not start one, so pfs started it outside the wall for repos that sign.
- **walls can't stack:** seatbelt refuses a second profile, so nothing that makes walls (the end-to-end script) could run inside a fork, and from inside one pfs couldn't start anything or run git in another fork.
- **and it only covered what pfs starts:** an in-process harness like pi-web, whose server was started elsewhere, was never inside any wall. that, and agents following AGENTS.md's words, ended it (session `01a13d07`).

## open questions

- `--context`: thoughts pending.
- telling a copy of a main from the original by path alone fails in one order: if the original moved and the copy runs pfs before it does, the copy takes over the forks (nothing is lost, they land in the copy). a random token in both `.main` and the record would settle it.

## build plan

1. ~~**v0, solo:**~~ done. `cargo install --path .`
2. ~~**`sandbox/`:**~~ done. `make-toy.sh` (toy repo), `hammer.sh` (end to end), `fake-agent.sh` (an agent stand-in, started like a real sub with `pfs run --bg`).
3. **pfs builds pfs:** the remaining features become tasks for pi subs in forks of this repo. review and merge by hand. every rough edge is the next fix.

## references

- pi session format (tree, `parentSession`): https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/session-format.md
- pi json event stream: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/json.md
- pi rpc (`steer`, `follow_up`): https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc-commands.md
- pi subagent example: https://github.com/earendil-works/pi/tree/main/packages/coding-agent/examples/extensions/subagent
