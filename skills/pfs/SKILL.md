---
name: pfs
description: Fan work out to parallel pfs forks (instant APFS clones of a repo, each on its own branch and ports) with headless agents (pi, or any harness that runs one task from the command line), then land it back through the main folder. Covers prepping the repo, splitting and writing sub tasks, the new -q → run --bg chain, waiting on subs (end your turn; their questions, reports and deaths wake you), reviewing, the rm → merge fan-in, relay checkpoints, fork-of-fork, and the etiquette that keeps parallel forks from colliding. Use when the user wants work split across agents or forks, subs/subagents on a repo, "pfs builds X", or to review and land what forks did.
---

# pfs: fan out, fan in

You orchestrate from the **main folder**. Subs work in forks. Work comes back only through `pfs rm` + a merge you do in main.

## The model

- **A fork is a folder** outside the repo, `~/.pfs/forks/<repo>/<n>` (`pfs path <n>` prints it): an APFS clone of the main folder as it is (node_modules, `.env`, uncommitted work and all). Nothing run in the main folder sees forks, so no tool needs a setting.
  - It has its own full copy of `.git` and its own branch `pfs/<n>-<name>`.
  - It gets ports `40000 + 10n …`, delivered as `$PORT`.
  - Nothing walls it. The rules are words: subs follow the repo's AGENTS.md (block below), and nothing is added to a sub's prompt but its task.
- **A sub is a background run:** `pfs run <n> --bg pi -p "<task>"`. pfs doesn't know it's an agent. Any harness that runs one task headless works the same (`claude -p`, `codex exec`).
  - A pi sub started from your bash is your **child**. With msg installed (its pi extension: `msg_list`, `msg_send`), it can ask you and wait for the answer, and its final message reaches you whole, as its report. It can message no one but you, and no one but you can message it.
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
pfs kill [<n>]                        # stop what's left running in fork n (a server a sub left); never its pfs runs, never you
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

### 2. Write each task: everything you know, on a plate

A sub starts cold, and its time goes to the model, not its commands: over 90% of a sub's wall time is model calls, while its commands, tests included, take 4-15%. So whatever you already found goes in the task, and the sub doesn't spend an hour finding it again:

- **The behavior, exactly:** CLI/API shape, output lines, exit codes, the edge cases you care about.
- **Where it goes:** the files and line ranges to change, the functions to reuse, the existing test or e2e scenario to copy.
- **What you already know:** facts you checked that it would otherwise re-check (which event fires when, what an API takes, a past bug), each with where you saw it.
- **The invariants:** e.g. "uncommitted must mean what rm means: share one function". Name the AGENTS.md rules that bite here.
- **Known traps:** name clashes ("`WaitArgs` already exists"), perf budgets with numbers ("20 forks under 0.5s"), files another fork is rewriting right now.
- **The checks, exactly:** the unit tests and the e2e scenarios to run while iterating (only the ones the change touches), then the full suite once before the report. You run everything again after merging. A sub's bash calls without a `timeout` stop after 5 min (msg's default), so give any check that can run longer its `timeout` (`bun run e2e` with `timeout: 900`).
- **End with:** "unit-test the pure parts. follow AGENTS.md." The sub commits on its branch.
- **Facts, not code.** Say where and what; the implementation is the sub's. Its report covers its judgment calls and the e2e checks main should add (AGENTS.md asks for that).

A task like that runs to a page: write it to a file outside the repo and pass `"$(cat <file>)"`.

### 3. Launch

One chain per task, from the main folder:

```sh
task='add `pfs wait [<n>...]`: …. unit-test the pure parts. follow AGENTS.md.'
n=$(pfs new wait -q) && r=$(pfs run "$n" --bg -q pi -p -n wait "$task")
```

- **Quote the task in single quotes** (or a variable, as above): backticks and `$` inside double quotes run in your shell.
- `-q` prints `5`, then `5.1`. Keep the run numbers: they name the fork too.
- **Lineage is automatic:** pi puts `PI_SESSION_ID` in your bash's env and pfs passes env through, so the sub knows you as its parent (msg) and `pfs new` records you as the fork's parent session. A wrapper that cleans the env must keep `PI_SESSION_ID`.
- **Name it** (`-n wait`): its messages carry the name, and the name is an address.
- **Nothing about msg goes in the task.** msg's tool guidelines and the AGENTS.md block already tell a sub to ask its parent when it's blocked. To hold a sub for a go-ahead, say so in the task: "ask your parent before committing" (it asks with `wait_seconds` and blocks until you answer).

### 4. Wait

- **pi subs: end your turn.** Their questions, reports, deaths and stalls wake you, so there's nothing to poll. pi-web keeps a parent with live subs loaded, and brings it back after a restart, so a sub that crashes still gets reported. The parent has to be interactive (pi-web or a tui): a `pi -p` parent exits when its turn ends. You're woken by:
  - **a question:** the sub asked with `wait_seconds` and is blocked until you answer. Answer with `msg_send` and `reply_to` its message id, then end your turn again.
  - **an `ended` report:** the sub's whole final message. Marked `stopped`, it was stopped mid-run (Esc, a kill): inspect its fork before landing anything. Review (5) and land (6) that one, then end your turn again for the rest.
  - **a `died` notice:** its process is gone without ending. `pfs ls` says how the run ended, `pfs logs <n>` what it printed.
  - **a `stalled` notice:** the sub is alive but stuck in one tool call past 10 min. With msg's 5-min default on a sub's bash, that's a call with a long `timeout` of its own, or another tool. The notice names the call and your options:
    - **Wait** if that's a legit long command: you won't hear about that call again, and its report still wakes you. `lookup` its session to see what it's doing.
    - **Stop just the command:** `kill -- -<pgid>`, printed when msg could pin the call's process group. The call comes back failed and the sub carries on, context intact. To redirect it, `msg_send` it what to do instead first: it reads that as soon as the call returns, before its next step.
    - **Stop the sub:** `kill <pid>`. pi takes the stuck command down with it, and the sub reports `ended`, stopped: inspect its fork before landing anything.
- **Not `pfs wait` for pi subs.** Nothing reaches you while you block in a bash command, so a sub that asks with `wait_seconds` sits idle until its own wait times out. `pfs wait` is for harnesses that can't message you (`claude -p`, `codex exec`): `pfs wait <n>.<run>...` prints one line per run as it settles (`5-wait   bg 5.1 done`), and a sub that already ended, even crashed in its first second, still reports how.
- **Steer:** `msg_send` to one sub by its exact id, its `-n` name or its fork's folder (`pfs path <n>`) once it's live, or to all of them with `to: "children"`. It lands at the sub's next step. Only you can message your subs. Answer and redirect; don't chat.
- **Resume** a stopped or dead sub in its fork, context and all: `pfs run <n> --bg -q pi -p --session <sub id> '<what to do now>'`. It's the same session, so its report wakes you as before; the cost is one cold re-read of its context. Start a new sub that `lookup`s the old session only when that context went bad (going in circles, the wrong approach).
- **Peek:** `pfs ls` (running, or how it ended) and `pfs diff <n> --stat`. `pi -p` prints only at the end, so its log is empty until then.
- **Ground truth:** msg reports what it saw; how a run ended is `pfs ls`.
- The conversation so far is the harness's business: pi files sessions by folder, so `cd "$(pfs path <n>)" && lookup last latest` reads it.

### 5. Review

- **The report:** the `ended` message, whole (only past 64KB does it point at lookup for the rest), or `pfs logs <n>`. With `pi -p` it's the sub's final message: its judgment calls and the e2e checks it lists.
- **The work:** `pfs ls` for `N commits · uncommitted`, then `pfs diff <n> --stat` and `pfs diff <n> -- <paths>`.
- **Its history:** `pfs run <n> git log --oneline main..HEAD` (the fork's `main` is main as it was at fork time).
- **Merge order:** `pfs ls --overlap` lists the files several forks changed.
- **Duplication:** parallel subs reinvent small helpers. Look for it and fold it together when merging.
- **A second reviewer is a fork too:** `pfs new review-x -q` (main as it is) or `--from pfs/<n>-x` (a fork's work before it lands), with a findings-only task. Its tree doesn't move while you merge, it has its own ports and its own copy of untracked data, and `pfs rm` drops it with no branch to save when it made no commits. A reviewer started in main itself reads code that changes under it and writes main's data.

### 6. Fan in

- **One at a time:** `pfs rm <n>`, read the note, merge, build and test, then the next.
  - The note reads `vs main right now: merges clean` or `2 files would conflict (a, b) · resolve when merging`.
  - Merge with `git merge --no-edit pfs/<n>-<name>`.
  - The note is a snapshot of main *now*. Re-read it after each merge.
- **All at once** (`pfs rm 3 5 7`) is fine when `--overlap` shows they don't share files.
- **"has uncommitted changes":** look with `pfs diff <n>`. Then commit it (`pfs run <n> git commit -am …`) or drop it with `--force`.
  - Main's wip that the fork was born with and never touched doesn't count.
- **"has someone working in it":** a process has its folder there (a server a sub left running in the background counts) or a pfs run is still going, one line each with what to do (`kill -- -<pid>` for a bg run, "exit that shell" for a door, `pfs kill <n>` for a process that's no run's). Wait for it or stop it. `--force` never skips this, and rm never kills anything.
- **"pi conversation … is mid-turn":** a sub or a pi-web chat is still thinking or running tools there. Wait. If it crashed (a turn left open forever), `--force`. A sub you killed doesn't count once its run has ended.
- **After the last merge:**
  1. Full tests, plus the e2e checks the subs listed.
  2. Push from main.
  3. `git branch -d pfs/…` the merged branches.

## Context: relay

With relay installed, every session, yours and each sub's, gets nudged at 40% context and every 5% after: finish the step, commit, call `relay` with a note. The reset is in place: same session id and file, a fresh context that opens with the goal, the user's messages, the note and a lookup catch-up.

- **You relay at checkpoints:** right after a merge and its checks, before you wait again. The note is the round's state: forks and what's in them, what's landed, what's next, the rules the user set.
- **Children don't notice.** Same id, so they keep their parent, and their questions and reports still reach you.
- **A nudged sub relays and keeps going** in the same `pi -p` run.
- **A parent's loop holds while its subs are live** (`/relay on <goal>`): it doesn't prompt you to keep going while you wait on them. And `/relay` sent mid-run waits for the run to finish.
- **Don't start subs in relay's loop** (`PI_RELAY_GOAL`): the loop ends with `relay` `done`, whose note msg doesn't read, so the `ended` report is whatever short line came last. A plain `pi -p` sub ends when its task is done, and its final message is the report.

## Fork of a fork

- **It's a main-folder move, never nesting:** forks are flat (`pfs new` fails inside a fork), and msg refuses a pi started from a sub's bash. A sub that needs more hands says so in its report.
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
- stop only what you started: `pfs kill` (what's left running in your fork), or by pid. never `pkill`/`killall` by pattern: every fork runs the same command lines, so a pattern stops other forks' servers too
- forks are flat: `pfs new` fails in here. if the task needs more hands, say so in your final message
- you may have a parent: the session that started you. blocked on a decision → `msg_send` to `"parent"` with `wait_seconds`. you can't message anyone else, and you never start pi sessions. never acknowledge. your final message is your report: your parent gets it whole when you end
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
- **A sub's `pkill -f` hits every fork.** Forks run the same command lines, so one sub's `pkill -INT -f "bun … serve"` also stopped the main folder's preview server and another fork's dev server. pfs keeps folders and ports apart, not processes: stop with `pfs kill` (it goes by folder) or by pid (the AGENTS.md block says so).
- **Untracked data is cloned at fork time, mid-write included.** A fork gets main's gitignored data too (a `var/*.sqlite`), which is handy: subs start with real data. But a clone taken while a server in main writes it can be torn (`pfs new` names whoever has files open for writing in the folder it copies), and a fork's writes to it die at `rm`, which lands commits only. Keep main's db quiet while you `pfs new`, point any server you run in main at a copy, and copy out what a fork wrote before you rm it.
- **Servers in main on hand-picked ports.** Main has no port block (`pfs port` there asks which fork), and the forks' blocks start at 40000 (fork 98 owns 40980-40989), so a preview server in main on 40980 sits in fork 98's block. Run main's servers below 40000.
- **Blocking in `pfs wait` on pi subs.** Their questions queue behind your bash call. End your turn instead: they wake you.
