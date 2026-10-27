#!/usr/bin/env bash
# Hammers pfs in a fresh toy repo: forks, runs and their waiters, rm,
# concurrency, a fake agent run like a real sub. PASS/FAIL per check; exits 1
# on any failure. Uses its own PFS_HOME (config and forks) and session dirs
# under sandbox/work, so it runs from a normal shell or from inside a pfs fork
# alike, and never touches your ~/.pfs.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$here/work"
toy="$work/toy"
mains="$work/mains"
PFS="${PFS:-$repo/target/debug/pfs}"

export PFS_HOME="$work/pfs-home"
export PI_CODING_AGENT_SESSION_DIR="$work/sessions"
export PI_CODING_AGENT_DIR="$work/agent"
unset PI_SESSION_ID PI_SESSION_FILE

passed=0
failed=0
check() {
  local name="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    passed=$((passed + 1))
    printf '  PASS  %s\n' "$name"
  else
    failed=$((failed + 1))
    printf '  FAIL  %s\n' "$name"
  fi
}
denied() { ! "$@"; }
contains() { case "$1" in *"$2"*) return 0 ;; *) return 1 ;; esac; }
section() { printf '\n%s\n' "$1"; }
info() { printf '  INFO  %s\n' "$1"; }
started_run() { sed -n 's/^started [0-9]*\.\([0-9]*\) .*/\1/p' <<<"$1"; }
started_pid() { sed -n 's/.*(pid \([0-9]*\)).*/\1/p' <<<"$1"; }
new_id() { sed -n 's/^created \([0-9]*\)-.*/\1/p' <<<"$1"; }
field() { tr -d ' ' <<<"$1"; }
real() { (cd "$1" && pwd -P); }
# numbered <forks folder>: how many forks are in it right now
numbered() { ls "$1" 2>/dev/null | grep -c '^[0-9][0-9]*$'; }
# repo <dir>: a bare-bones main with one commit
repo() { mkdir -p "$1" && git -C "$1" init -q -b main && git -C "$1" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init; }
# fake_conversation <file> <fork dir> <id> <last message>: a pi session file
# whose first line puts it in that fork, ending on that message.
fake_conversation() {
  mkdir -p "$(dirname "$1")"
  printf '{"type":"session","version":3,"id":"%s","timestamp":"2026-01-01T00:00:00.000Z","cwd":"%s"}\n{"type":"message","message":{"role":"user","content":"go"}}\n{"type":"message","message":%s}\n' "$3" "$(real "$2")" "$4" >"$1"
}
tool_use='{"role":"assistant","content":[],"stopReason":"toolUse"}'
tool_result='{"role":"toolResult","content":[]}'
stopped='{"role":"assistant","content":[],"stopReason":"stop"}'

(cd "$repo" && cargo build -q) || exit 1
rm -rf "$PFS_HOME" "$mains" "$work/sessions" "$work/agent" "$work/scratch" "$work/repo-cache" "$work/global-cache"
mkdir -p "$PI_CODING_AGENT_SESSION_DIR" "$work/scratch"
agent="$here/fake-agent.sh"
"$here/make-toy.sh" >/dev/null || exit 1
cd "$toy" || exit 1
ledger=.git/pfs/ledger.jsonl

section "pfs new"
exclude_before=$(cat .git/info/exclude 2>/dev/null)
echo "wip" >>README.md
t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
out=$("$PFS" new add-csv)
t1=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
info "$out ($(echo "$t1 - $t0" | bc)s)"
forks=$(cat .git/pfs/forks-dir)
check "main records its forks folder: <forks dir>/toy" test "$forks" = "$(real "$PFS_HOME")/forks/toy"
check "fork 1 created there" test -d "$forks/1"
check "  …and new says where" contains "$out" "/pfs-home/forks/toy/1 (branch pfs/1-add-csv"
check "the toy has no .forks" test ! -e "$toy/.forks"
check "  …and nothing new in .git/info/exclude" test "$(cat .git/info/exclude 2>/dev/null)" = "$exclude_before"
check "the forks folder's .main names the toy" test "$(cat "$forks/.main")" = "$(real "$toy")"
check "  …and its README.md says to remove forks with pfs rm, never by hand" grep -q 'pfs rm <n>.*never by hand' "$forks/README.md"
check "PFS_HOME, the forks dir and the toy's folder are 0700" test "$(stat -f %Lp "$PFS_HOME" "$PFS_HOME/forks" "$forks" | sort -u)" = 700
check "on branch pfs/1-add-csv" test "$(git -C "$forks/1" branch --show-current)" = pfs/1-add-csv
check "node_modules came along" test -d "$forks/1/node_modules"
check ".env came along" test -f "$forks/1/.env"
check "venv came along" test -x "$forks/1/.venv/bin/python"
check "uncommitted main work came along" grep -q wip "$forks/1/README.md"
check "  …and new says so, naming it" grep -qE "note: main's uncommitted (README.md came along|work came along \([0-9]+ files: [^)]*README.md)" <<<"$out"
check "ledger has fork 1" grep -q '"type":"fork","id":1,' "$ledger"
check "the fork has no ledger or forks record of its own" test ! -e "$forks/1/.git/pfs/ledger.jsonl" -a ! -e "$forks/1/.git/pfs/forks-dir"
check "  …and no .forks" test ! -e "$forks/1/.forks"
check "diff is empty at start (main's wip isn't the fork's change)" test -z "$("$PFS" diff 1)"
git checkout -q README.md

"$PFS" new clean --from HEAD~1 >/dev/null
check "--from starts at that commit" test "$(git -C "$forks/2" rev-parse HEAD)" = "$(git rev-parse HEAD~1)"
check "--from drops tracked changes" denied grep -q "step 2" "$forks/2/README.md"
check "--from keeps ignored files (.env)" test -f "$forks/2/.env"

out=$("$PFS" new 7-add-csv)
check "a leading number is pfs's, not the label's" contains "$out" "created 3-add-csv"
before=$(numbered "$forks")
check "a bad --from fails cleanly" denied "$PFS" new nope --from no-such-rev
check "  …and leaves no folder" test "$(numbered "$forks")" -eq "$before"
check "a bare number isn't a name" denied "$PFS" new 5
check "  …the error points at --of and open" contains "$("$PFS" new 5 2>&1)" "pfs new --of 5"
check "  …and leaves no folder" test "$(numbered "$forks")" -eq "$before"

section "pfs path"
check "pfs path 1 is fork 1's folder" test "$("$PFS" path 1)" = "$forks/1"
check "  …so cd \"\$(pfs path 1)\" works" test "$(cd "$("$PFS" path 1)" && git branch --show-current)" = pfs/1-add-csv
check "inside a fork, the number is optional" test "$(cd "$forks/1/node_modules" && "$PFS" path)" = "$forks/1"
check "  …and another fork's works too" test "$(cd "$forks/1" && "$PFS" path 2)" = "$forks/2"
check "in main, it needs a number" denied "$PFS" path
check "an unknown fork is an error" denied "$PFS" path 99
check "  …saying so" contains "$("$PFS" path 99 2>&1)" "no fork 99"

section "runs"
check "a command writes in its fork" "$PFS" run 1 sh -c 'echo ok > inside.txt'
check "  …landed" test -f "$forks/1/inside.txt"
check "git commit inside" "$PFS" run 1 git commit -qm "fix: two words" --allow-empty
check "  …argv arrives intact" test "$(git -C "$forks/1" log -1 --format=%s)" = "fix: two words"
"$PFS" run 1 sh -c 'exit 7' >/dev/null 2>&1
code=$?
check "exit codes pass through (7)" test "$code" -eq 7
check "\$PORT is the fork's (3 → 40030)" test "$("$PFS" run 3 sh -c 'echo $PORT')" = 40030
check "pfs port inside fork 1" test "$(cd "$forks/1" && "$PFS" port)" = 40010
check "pfs port in main needs a number" denied "$PFS" port
check "pfs port 5 from main" test "$("$PFS" port 5)" = 40050
check "foreground runs are recorded" grep -q '"kind":"fg"' "$forks/1/.git/pfs/runs.jsonl"
check "unknown commands fail early" denied "$PFS" run 1 no-such-cmd-xyz
check "inside a subfolder, runs start there" test "$(cd "$forks/1/node_modules" && "$PFS" run pwd)" = "$forks/1/node_modules"

section "background runs"
out=$("$PFS" run 1 --bg 'echo hello; sleep 2; echo bye')
run=$(started_run "$out")
check "started" test -n "$run"
check "pfs ls shows it" contains "$("$PFS" ls)" "bg 1.$run running"
sleep 3
check "exit recorded by the waiter" grep -q "\"type\":\"exit\",\"run\":$run,\"code\":0" "$forks/1/.git/pfs/runs.jsonl"
check "pfs logs has the output" test "$("$PFS" logs "1.$run")" = "$(printf 'hello\nbye')"
check "the log lives in .git/pfs" test -f "$forks/1/.git/pfs/$run.log"
check "git doesn't see logs" test -z "$(git -C "$forks/1" status --porcelain | grep '\.log')"
out=$("$PFS" run 1 --bg 'for i in 1 2 3; do echo tick $i; sleep 0.4; done')
check "pfs logs -f follows to the end" test "$("$PFS" logs -f "1.$(started_run "$out")" | wc -l | tr -d ' ')" -eq 3

out=$("$PFS" run 1 --bg sleep 30)
pid=$(started_pid "$out")
waiter=$(field "$(ps -o ppid= -p "$pid")")
check "the command leads its own process group" test "$(field "$(ps -o pgid= -p "$pid")")" = "$pid"
check "the waiter is outside our process group" test "$(field "$(ps -o pgid= -p "$waiter")")" != "$(field "$(ps -o pgid= -p $$)")"
check "the waiter is a session leader (setsid)" test "$(field "$(ps -o pgid= -p "$waiter")")" = "$waiter"
kill -9 "$waiter"
sleep 0.3
check "kill -9 the waiter: the command lives on" kill -0 "$pid"
check "  …pfs ls still says running" contains "$("$PFS" ls)" "running"
kill -- -"$pid"
sleep 0.5
check "  …then it shows as died (nobody wrote an exit)" contains "$("$PFS" ls --json)" '"status":"died"'

perl -e 'setpgrp(0,0); system(@ARGV); kill("KILL", -getpgrp())' "$PFS" run 1 --bg sleep 20 >"$work/pgkill.out" 2>&1
pid=$(started_pid "$(cat "$work/pgkill.out")")
check "killing the caller's process group (pi timeout/esc) spares the run" kill -0 "$pid"
kill -- -"$pid" 2>/dev/null
sleep 0.3
check "  …and the waiter records the kill" grep -q '"signal":15' "$forks/1/.git/pfs/runs.jsonl"

section "pfs rm"
out=$("$PFS" run 1 --bg sleep 30)
pid=$(started_pid "$out")
check "refuses while something runs" denied "$PFS" rm 1
check "  …and says how to stop it" contains "$("$PFS" rm 1 2>&1)" "kill -- -$pid"
kill -- -"$pid"
sleep 0.5
check "refuses uncommitted changes" denied "$PFS" rm 1
"$PFS" run 1 sh -c 'git add -A && git commit -qm "agent work"' >/dev/null
out=$("$PFS" rm 1)
code=$?
check "works once committed" test "$code" -eq 0
check "  …with a plain merge note" contains "$out" "vs main right now: merges clean"
check "  …branch saved in main" git rev-parse -q --verify refs/heads/pfs/1-add-csv
check "  …folder gone" test ! -e "$forks/1"
check "  …ledger rm line" grep -q '"type":"rm","id":1,' "$ledger"
check "  …no trash left" test -z "$(ls -A "$forks" | grep '^\.rm-')"
check "pfs path of a removed fork is an error" denied "$PFS" path 1
check "  …saying it was removed" contains "$("$PFS" path 1 2>&1)" "fork 1 was removed"
check "pfs ls --all lists it" contains "$("$PFS" ls --all)" "removed"
check "  …with its saved branch" grep -q "1-add-csv .*saved pfs/1-add-csv$" <<<"$("$PFS" ls --all)"
git branch -q -m pfs/1-add-csv hammer-aside
check "  …and says when that branch is gone" grep -q "saved pfs/1-add-csv, deleted since$" <<<"$("$PFS" ls --all)"
git branch -q -m hammer-aside pfs/1-add-csv
check "a removed fork gives a useful error" contains "$("$PFS" run 1 true 2>&1)" "was removed"
out=$("$PFS" new after-rm)
check "ids are never reused" contains "$out" "created 4-after-rm"
echo scratch >"$forks/4/scratch.txt"
check "--force drops uncommitted work" "$PFS" rm 4 --force
check "  …no commits, so no branch saved" denied git rev-parse -q --verify refs/heads/pfs/4-after-rm
check "pfs rm needs the main folder" denied sh -c "cd '$forks/2' && '$PFS' rm 3"

section "pfs diff"
(cd "$forks/2" && echo changed >>README.md && echo new >untracked.txt)
"$PFS" run 2 sh -c 'echo c > committed.txt && git add committed.txt && git commit -qm c' >/dev/null
diff_stat=$("$PFS" diff 2 --stat)
check "shows uncommitted edits" contains "$diff_stat" "README.md"
check "shows untracked files" contains "$diff_stat" "untracked.txt"
check "shows commits" contains "$diff_stat" "committed.txt"
check "takes pathspecs after --" test "$("$PFS" diff 2 --name-only -- README.md)" = "README.md"
check "leaves the fork's real index alone" test -z "$(git -C "$forks/2" diff --cached --name-only)"

section "git in a fork"
check "git stash and branches work" "$PFS" run 2 sh -c 'echo s > stash.txt && git add stash.txt && git stash -q && git switch -q -c side && git switch -q pfs/2-clean && git stash pop -q && git reset -q && rm stash.txt'
check "git config works: a fork's settings are its own" "$PFS" run 2 git config pfs.probe yes
check "  …and never reach main" denied git config --get pfs.probe
git -C "$forks/2" config --unset pfs.probe
printf 'notes\n' >main-wip.txt

section "from inside a fork"
check "pfs ls works" "$PFS" run 3 "$PFS" ls
check "  …and marks the fork you're in" contains "$("$PFS" run 3 "$PFS" ls)" "* 3-add-csv"
check "  …and checks every fork's work (no ?)" denied contains "$("$PFS" run 3 "$PFS" ls)" "?"
check "pfs new fails: forks are flat" denied "$PFS" run 3 "$PFS" new nested
check "pfs rm fails: it runs from the main folder" denied "$PFS" run 3 "$PFS" rm 2
check "pfs run --bg works" contains "$("$PFS" run 3 "$PFS" run --bg 'echo inner')" "started 3."
sleep 0.5
check "  …and its log is readable" contains "$("$PFS" logs 3)" "inner"
check "pfs run into another fork works" test "$("$PFS" run 3 "$PFS" run 2 pwd)" = "$forks/2"
check "pfs diff into another fork works" contains "$("$PFS" run 3 "$PFS" diff 2 --stat)" "committed.txt"
check "pfs open into another fork works" contains "$(printf 'pwd\n' | SHELL=/bin/sh "$PFS" run 3 "$PFS" open 2 2>&1)" "$forks/2"
check "pfs port" test "$("$PFS" run 3 "$PFS" port)" = 40030
check "pfs diff" "$PFS" run 3 "$PFS" diff

section "no walls: old settings are ignored"
check "pfs allow is gone" denied "$PFS" allow
check "  …and so is pfs new --allow" denied "$PFS" new x --allow /tmp
check "runs get no git settings from pfs" test "$("$PFS" run 3 sh -c 'env | grep ^GIT_CONFIG | sort')" = "$(env | grep ^GIT_CONFIG | sort)"
printf '[wall]\nallow = ["~/.m2"]\nlock_git_settings = false\n\n[ports]\nbase = 41000\n' >"$PFS_HOME/config.toml"
check "an old [wall] table in the config is ignored, its [ports] still read" test "$("$PFS" port 5)" = 41050
rm "$PFS_HOME/config.toml"
printf '[wall]\nallow = ["~", "../toy"]\nlock_git_settings = true\n' >.pfs.toml
check "an old .pfs.toml is ignored" test -z "$("$PFS" run 3 true 2>&1)"
check "  …git config works in a fork" "$PFS" run 3 git config pfs.probe yes
git -C "$forks/3" config --unset pfs.probe
rm .pfs.toml
printf '{"type":"allow","id":3,"path":"/opt/cache","ts":"2026-01-01T00:00:00.000Z"}\n' >>"$ledger"
check "an old allow line in the ledger changes nothing" contains "$("$PFS" ls)" "3-add-csv"

section "20 forks at once"
before=$(grep -c '"type":"fork"' "$ledger")
live=$(numbered "$forks")
t0=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
seq 1 20 | xargs -P 20 -I{} "$PFS" new burst-{} >/dev/null 2>&1
t1=$(perl -MTime::HiRes=time -e 'printf "%.3f", time')
info "20 parallel pfs new: $(echo "$t1 - $t0" | bc)s"
after=$(grep -c '"type":"fork"' "$ledger")
check "20 new fork lines" test $((after - before)) -eq 20
check "no id twice" test -z "$(jq -r 'select(.type=="fork") | .id' "$ledger" | sort | uniq -d)"
check "every ledger line parses" jq -e . "$ledger"
burst=$(jq -r 'select(.type=="fork" and (.name // "" | startswith("burst"))) | .id' "$ledger")
rms_before=$(grep -c '"type":"rm"' "$ledger")
echo "$burst" | xargs -P 20 -I{} "$PFS" rm {} >/dev/null 2>&1
check "20 parallel rm" test $(($(grep -c '"type":"rm"' "$ledger") - rms_before)) -eq 20
check "  …folders gone" test "$(numbered "$forks")" -eq "$live"

section "a sub is a chained bg run (fake pi)"
id=$(PI_SESSION_ID=parent-0001 "$PFS" new agent-test -q)
run=$("$PFS" run "$id" --bg -q "$agent" -p "write a note sleep 1")
check "started as $id.1" test "$run" = "$id.1"
check "pfs ls: bg running" contains "$("$PFS" ls)" "bg $id.1 running"
sleep 2.5
check "pfs ls: how it ended, once it ended" contains "$("$PFS" ls)" "bg $id.1 done"
log=$("$PFS" logs "$id")
check "it worked in its fork" contains "$log" "cwd=$forks/$id"
check "it got its port" contains "$log" "port=$("$PFS" port "$id")"
check "it got its task" contains "$log" "task=write a note sleep 1"
check "it committed on its branch" test "$(git -C "$forks/$id" log -1 --format=%s)" = "fake agent: write a note sleep 1"
check "the parent session is in the ledger" grep -q "\"id\":$id,.*\"parent_session\":\"parent-0001\"" "$ledger"
check "pfs logs is its output, nothing about lookup" denied contains "$log" "lookup"
check "--agent is gone" denied "$PFS" new gone --agent "x"
printf '{"type":"start","run":7,"kind":"agent","cmd":"pi: old","pid":1,"session":"s","ts":"2026-01-01T00:00:00.000Z"}\n{"type":"exit","run":7,"code":0,"ts":"2026-01-01T00:01:00.000Z"}\n' >>"$forks/$id/.git/pfs/runs.jsonl"
check "an old agent run still reads, as bg" contains "$("$PFS" ls)" "bg $id.7 done"
check "  …and its log is a bg run's" contains "$("$PFS" logs "$id.7" 2>&1)" "can't open"

section "pfs open (the door)"
door=$(printf 'echo "port=$PORT"; pwd\n' | SHELL=/bin/sh "$PFS" open 3 2>&1)
check "a shell in the fork" contains "$door" "$forks/3"
check "  …with the fork's port" contains "$door" "port=40030"
check "  …says where you are" contains "$door" "fork 3-add-csv · branch pfs/3-add-csv · ports 40030-40039"
check "  …and nothing about walls" denied contains "$door" "wall"
check "  …recorded as an open run" grep -q '"kind":"open"' "$forks/3/.git/pfs/runs.jsonl"
check "pfs open in main needs a number" denied "$PFS" open
id=$("$PFS" new door-agent -q)
"$PFS" run "$id" --bg -q "$agent" -p "wait sleep 3" >/dev/null
door=$(echo exit | SHELL=/bin/sh "$PFS" open "$id" 2>&1)
check "it says what's still running there" contains "$door" "still running in this fork: $id.1 bg"
check "  …and nothing about pi or sessions" denied grep -qwiE 'pi|session' <<<"$door"
sleep 3.5

section "pfs new --of (a fork of a fork, folders stay flat)"
echo "half-done" >"$forks/3/wip.txt"
out=$("$PFS" new of-three --of 3)
id=$(new_id "$out")
check "created" test -n "$id"
check "  …says what it copied" contains "$out" "a copy of fork 3-add-csv as it is now"
check "  …lives flat, next to fork 3" test -d "$forks/$id"
check "  …has fork 3's uncommitted work" test -f "$forks/$id/wip.txt"
check "  …starts at fork 3's commit" test "$(git -C "$forks/$id" rev-parse HEAD)" = "$(git -C "$forks/3" rev-parse HEAD)"
check "  …on its own branch" test "$(git -C "$forks/$id" branch --show-current)" = "pfs/$id-of-three"
check "  …diff is empty at start" test -z "$("$PFS" diff "$id")"
check "  …the ledger knows it's of 3" grep -q "\"id\":$id,.*\"of\":3" "$ledger"
check "  …pfs ls shows it" contains "$("$PFS" ls)" "of 3"
check "fork 3 is untouched" test "$(git -C "$forks/3" branch --show-current)" = pfs/3-add-csv
check "--of and --from don't mix" denied "$PFS" new x --of 3 --from HEAD
check "--of a fork that doesn't exist fails" denied "$PFS" new x --of 99
rm "$forks/3/wip.txt"
"$PFS" rm "$id" --force >/dev/null

section "pfs new --from a live fork's branch"
"$PFS" run 3 sh -c 'echo live > live.txt && git add live.txt && git commit -qm "live work"' >/dev/null
out=$("$PFS" new from-live --from pfs/3-add-csv)
id=$(new_id "$out")
check "works while fork 3 is open" test -n "$id"
check "  …at fork 3's latest commit" test "$(git -C "$forks/$id" rev-parse HEAD)" = "$(git -C "$forks/3" rev-parse HEAD)"
check "  …says where it came from" contains "$out" "taken from its fork"
check "  …main didn't get the branch" denied git rev-parse -q --verify refs/heads/pfs/3-add-csv
check "  …no leftover ref" denied git -C "$forks/$id" rev-parse -q --verify refs/pfs/incoming
check "  …pfs ls shows it" contains "$("$PFS" ls)" "from pfs/3-add-csv"
"$PFS" rm "$id" --force >/dev/null

section "pfs rm's merge note"
out=$("$PFS" new clash)
id=$(new_id "$out")
"$PFS" run "$id" sh -c 'echo fork-side > README.md && git commit -qam "fork edits readme"' >/dev/null
echo main-side >README.md && git commit -qam "main edits readme"
out=$("$PFS" rm "$id" --force)
check "names the conflicting file, calmly" contains "$out" "vs main right now: 1 file would conflict (README.md) · resolve when merging"
git reset -q --hard HEAD~1
git branch -q -D "pfs/$id-clash"
before=$(git rev-parse HEAD)
echo main-wip >>README.md
id=$(new_id "$("$PFS" new swept)")
"$PFS" run "$id" sh -c 'echo f >f.txt && git add -A && git commit -qm "fork work, main wip swept in"' >/dev/null
out=$("$PFS" rm "$id")
check "main's own wip in the merge's way (tracked or not): the note says so" grep -q "merges clean, once [0-9]* files uncommitted here (README.md, .*main-wip.txt) are committed or stashed$" <<<"$out"
check "  …as git would" denied git merge --no-edit -q "pfs/$id-swept"
git reset -q --hard "$before"
git branch -q -D "pfs/$id-swept"

section "pfs ls: work to land"
out=$("$PFS" new landing)
id=$(new_id "$out")
row() { "$PFS" ls | grep " $1-"; }
check "a fresh fork has nothing to land" denied contains "$(row "$id")" "commit"
"$PFS" run "$id" sh -c 'echo one > one.txt && git add one.txt && git commit -qm one && echo two > two.txt && git add two.txt && git commit -qm two' >/dev/null
check "its commits show" contains "$(row "$id")" "2 commits"
check "  …main's untracked main-wip.txt it carries isn't its uncommitted work" denied contains "$(row "$id")" "uncommitted"
echo edit >>"$forks/$id/one.txt"
check "an edit after the commits shows" contains "$(row "$id")" "2 commits · uncommitted"
check "  …in --json too" test "$("$PFS" ls --json | jq -r "select(.id==$id) | \"\\(.commits) \\(.uncommitted)\"")" = "2 true"
check "rm refuses what ls calls uncommitted" denied "$PFS" rm "$id"
"$PFS" run "$id" git checkout -q one.txt
check "  …and takes it once that's undone" "$PFS" rm "$id"
check "  …landing only the fork's own files" test "$(git show --name-only --format= "pfs/$id-landing" pfs/"$id"-landing~1 | sort | tr '\n' ' ')" = "one.txt two.txt "
git branch -q -D "pfs/$id-landing"

section "pfs rm with several forks"
a=$(new_id "$("$PFS" new many-a)")
b=$(new_id "$("$PFS" new many-b)")
c=$(new_id "$("$PFS" new many-c)")
echo dirty >"$forks/$b/dirty.txt"
out=$("$PFS" rm "$a" "$b" "$c" 2>&1)
code=$?
check "removes the ones it can" test ! -e "$forks/$a" -a ! -e "$forks/$c"
check "  …keeps the refused one" test -d "$forks/$b"
check "  …says why and exits 1" test "$code" -eq 1 -a -n "$(grep "fork $b-many-b has uncommitted changes" <<<"$out")"
check "  …and sums up what's left" contains "$out" "1 of 3 not removed ($b)"
"$PFS" rm "$b" --force >/dev/null

section "pfs rm: is anyone working here"
id=$("$PFS" new busy -q)
mkdir -p "$forks/$id/src" "$forks/${id}0x"
(cd "$forks/$id/src" && exec sleep 30) &
spid=$!
(cd "$forks/${id}0x" && exec sleep 30) &
npid=$!
sleep 0.3
out=$("$PFS" rm "$id" 2>&1)
check "a process with its folder in the fork blocks rm" contains "$out" "pid $spid  sleep  (stop: pfs kill $id)"
check "  …one in a folder whose name only starts the same doesn't" denied contains "$out" "pid $npid"
check "  …--force doesn't skip it" denied "$PFS" rm "$id" --force
check "  …the fork is still there" test -d "$forks/$id"
check "  …and nothing was killed" kill -0 "$spid"
kill "$spid" "$npid" 2>/dev/null
wait "$spid" "$npid" 2>/dev/null
rmdir "$forks/${id}0x"
check "  …once it's gone, rm goes through" "$PFS" rm "$id"

id=$("$PFS" new talking -q)
conv="$PI_CODING_AGENT_SESSION_DIR/hammer-$id.jsonl"
fake_conversation "$conv" "$forks/$id" "0a1b2c3d-open" "$tool_use"
out=$("$PFS" rm "$id" 2>&1)
check "a pi conversation mid-turn in the fork blocks rm" contains "$out" "pi conversation 0a1b2c3d is mid-turn (running tools, last write"
check "  …and says what to do if it crashed" contains "$out" "if it crashed: pfs rm $id --force"
printf '{"type":"message","message":%s}\n' "$stopped" >>"$conv"
check "  …closed, rm goes through" "$PFS" rm "$id"
id=$("$PFS" new web -q)
enc=$(real "$forks/$id" | sed 's|^/||; s|[/\\:]|-|g')
fake_conversation "$PI_CODING_AGENT_DIR/sessions/--$enc--/web.jsonl" "$forks/$id" "0a1b2c3e-web" "$tool_result"
check "one filed the way pi and pi-web file them (sessions/--<path>--) counts too" contains "$("$PFS" rm "$id" 2>&1)" "pi conversation 0a1b2c3e is mid-turn (thinking"
check "a conversation a crash left open: --force goes through" "$PFS" rm "$id" --force

id=$("$PFS" new killed -q)
pid=$(started_pid "$("$PFS" run "$id" --bg "$agent" -p "converse sleep 30")")
sleep 0.5
conv=$(grep -l "\"cwd\":\"$(real "$forks/$id")\"" "$PI_CODING_AGENT_SESSION_DIR"/fake-*.jsonl)
check "a fake sub keeps its conversation where pi would" test -n "$conv"
out=$("$PFS" rm "$id" --force 2>&1)
check "a sub at work blocks rm, --force or not" contains "$out" "$id.1 bg pid $pid"
check "  …as one line, its processes folded in" test "$(grep -c "pid " <<<"$out")" -eq 1
kill -- -"$pid"
sleep 0.5
check "killed mid-turn, the turn it left open doesn't block rm" "$PFS" rm "$id"
id=$("$PFS" new killed-touched -q)
pid=$(started_pid "$("$PFS" run "$id" --bg "$agent" -p "converse sleep 30")")
sleep 0.5
conv=$(grep -l "\"cwd\":\"$(real "$forks/$id")\"" "$PI_CODING_AGENT_SESSION_DIR"/fake-*.jsonl)
kill -- -"$pid"
sleep 1.5
touch "$conv"
check "  …unless it was written to after the run ended" contains "$("$PFS" rm "$id" 2>&1)" "is mid-turn"
"$PFS" rm "$id" --force >/dev/null
id=$("$PFS" new died -q)
printf '{"type":"start","run":1,"kind":"bg","cmd":"pi -p x","pid":999999,"ts":"2026-01-01T00:00:00.000Z"}\n' >>"$forks/$id/.git/pfs/runs.jsonl"
fake_conversation "$PI_CODING_AGENT_SESSION_DIR/hammer-$id.jsonl" "$forks/$id" "0a1b2c3f-died" "$tool_use"
check "a died run (no exit line) clears nothing" contains "$("$PFS" rm "$id" 2>&1)" "is mid-turn"
check "  …until --force" "$PFS" rm "$id" --force

a=$("$PFS" new free -q)
b=$("$PFS" new held -q)
(cd "$forks/$b" && exec sleep 30) &
hpid=$!
sleep 0.3
"$PFS" rm "$a" "$b" >/dev/null 2>&1
code=$?
check "pfs rm a b with only b occupied removes a and keeps b" test ! -e "$forks/$a" -a -d "$forks/$b" -a "$code" -eq 1
kill "$hpid" 2>/dev/null
wait "$hpid" 2>/dev/null
"$PFS" rm "$b" >/dev/null

id=$("$PFS" new doorway -q)
(sleep 2 | SHELL=/bin/sh "$PFS" open "$id" >/dev/null 2>&1) &
dpid=$!
sleep 0.5
out=$("$PFS" rm "$id" 2>&1)
check "an open door blocks rm" contains "$out" "$id.1 open pid"
check "  …and says to exit that shell" contains "$out" "(exit that shell)"
check "  …never to kill it" denied contains "$out" "kill"
wait "$dpid"
inside=$(printf 'cd "%s" && "%s" rm %s\n' "$toy" "$PFS" "$id" | SHELL=/bin/sh "$PFS" open "$id" 2>&1)
check "rm typed inside the door says it's the door you're in" contains "$inside" "that's the door you're in: exit it, then rm from a normal shell"
check "  …and keeps the fork" test -d "$forks/$id"
"$PFS" rm "$id" >/dev/null

section "chaining: new -q, run --bg -q"
qid=$("$PFS" new 98-quiet -q 2>"$work/quiet.err")
check "new -q prints only the fork number" grep -qx '[0-9][0-9]*' <<<"$qid"
check "  …and that's the fork it made" test "$(git -C "$forks/$qid" branch --show-current)" = "pfs/$qid-quiet"
check "  …notes go to stderr" grep -q 'became "quiet"' "$work/quiet.err"
qrun=$("$PFS" run "$qid" --bg -q sleep 0.2)
check "run --bg -q prints only <n>.<run>" test "$qrun" = "$qid.1"
qrun=$("$PFS" run -q "$qid" --bg sleep 0.2)
check "  …-q before the fork number too" test "$qrun" = "$qid.2"
cid=$("$PFS" new chain -q) && crun=$("$PFS" run "$cid" --bg -q echo chained) && "$PFS" wait "$crun" >/dev/null
check "  …so a chain needs no parsing" test "$("$PFS" logs "$crun")" = chained
check "new -q and --open don't mix" denied "$PFS" new x -q --open
"$PFS" wait "$qid" >/dev/null
"$PFS" rm "$qid" "$cid" --force >/dev/null

section "pfs wait"
out=$("$PFS" new waiting)
id=$(new_id "$out")
check "nothing running: returns at once, 0" "$PFS" wait "$id"
out=$("$PFS" wait "$id")
check "  …saying just that, when it never had a bg run" test "${out##*   }" = "nothing was running"
"$PFS" run "$id" --bg sleep 2 >/dev/null
"$PFS" run "$id" --bg 'sleep 1; exit 3' >/dev/null
t0=$(date +%s)
out=$("$PFS" wait "$id")
code=$?
check "waits for bg runs, exits 1 on a failure" test "$code" -eq 1
check "  …until the slowest ended" test $(($(date +%s) - t0)) -ge 1
check "  …one line saying how each ended" contains "$out" "failed (exit 3)"
out=$("$PFS" run "$id" --bg sleep 5)
pid=$(started_pid "$out")
out=$("$PFS" wait "$id" --timeout 1 2>&1)
code=$?
check "--timeout gives up with 124" test "$code" -eq 124
check "  …saying what's still running" contains "$out" "still running"
kill -- -"$pid" 2>/dev/null
aid=$("$PFS" new wait-agent -q)
"$PFS" run "$aid" --bg -q "$agent" -p "a note sleep 1" >/dev/null
out=$("$PFS" wait "$aid")
code=$?
check "waits for a sub" contains "$out" "bg $aid.1 done"
check "  …exit 0" test "$code" -eq 0
"$PFS" run "$id" --bg 'sleep 0.2; exit 4' >/dev/null
"$PFS" wait "$id" >/dev/null
check "pfs ls: a failed bg run says so" contains "$("$PFS" ls)" "bg $id.4 failed (exit 4)"
out=$("$PFS" run "$id" --bg sleep 30)
pid=$(started_pid "$out")
kill -9 "$(field "$(ps -o ppid= -p "$pid")")" && kill -- -"$pid"
sleep 0.5
check "pfs ls: a death says when it started" contains "$("$PFS" ls)" "bg $id.5 died, started"
r=$("$PFS" run "$id" --bg -q 'exit 1')
sleep 0.5
out=$("$PFS" wait "$r")
code=$?
check "a run that already ended, waited on by number, says how" contains "$out" "bg $r failed (exit 1)"
check "  …and exits 1" test "$code" -eq 1
out=$("$PFS" wait "$id")
code=$?
check "a fork with nothing running says how its last bg run ended" contains "$out" "nothing was running · bg $r failed (exit 1)"
check "  …but only watched runs count: exit 0" test "$code" -eq 0
long=$(started_pid "$("$PFS" run "$id" --bg sleep 5)")
short=$("$PFS" run "$id" --bg -q sleep 0.2)
t0=$(date +%s)
out=$("$PFS" wait "$short")
check "a run target waits on that run, not its siblings" test $(($(date +%s) - t0)) -lt 3
check "  …and says it's done" contains "$out" "bg $short done"
kill -- -"$long" 2>/dev/null
check "a run that isn't there is an error" denied "$PFS" wait "$id.99"
check "  …naming it" contains "$("$PFS" wait "$id.99" 2>&1)" "has no background run $id.99"
check "a whole fork covers its runs named with it" test "$("$PFS" wait "$id" "$r" | wc -l | tr -d ' ')" = 1
check "a gone fork is an error" denied "$PFS" wait 999

section "pfs ls --overlap, lineage in briefs, cross-fork git"
ova=$(new_id "$("$PFS" new ov-a)")
ovb=$(new_id "$("$PFS" new ov-b)")
ovc=$(new_id "$("$PFS" new ov-c)")
"$PFS" run "$ova" sh -c 'echo a >> README.md && git commit -qam "a edits readme"' >/dev/null
echo b >>"$forks/$ovb/README.md"
echo c >"$forks/$ovc/c.txt"
overlap=$("$PFS" ls --overlap)
check "a file two forks changed is listed with both" grep -q "README.md.*$ova-ov-a.*$ovb-ov-b" <<<"$overlap"
check "  …a file only one fork changed isn't" denied grep -q "c.txt" <<<"$overlap"
check "  …nor main's carried, untouched wip" denied grep -q "main-wip.txt" <<<"$overlap"
check "  …--json gets it with --overlap" contains "$("$PFS" ls --json --overlap | jq -r "select(.id==$ova) | .overlap | keys | join(\",\")")" "README.md"
check "  …and not without" test "$("$PFS" ls --json | jq -s 'map(has("overlap")) | any')" = false
inside=$("$PFS" run "$ova" "$PFS" ls --overlap)
check "inside a fork, --overlap sees every fork too" grep -q "README.md.*$ova-ov-a.*$ovb-ov-b" <<<"$inside"
check "  …and skips none" denied contains "$inside" "skipped"
check "pfs diff into another fork from inside one works" contains "$("$PFS" run "$ova" "$PFS" diff "$ovb" --stat)" "README.md"
check "  …and so does pfs run" test "$("$PFS" run "$ova" "$PFS" run "$ovb" pwd)" = "$forks/$ovb"
frid=$("$PFS" new ov-from --from HEAD~1 -q)
check "pfs ls names the commit a moving --from meant" contains "$("$PFS" ls)" "from HEAD~1 ($(git rev-parse --short=7 HEAD~1))"
"$PFS" rm "$ova" "$ovb" "$ovc" "$frid" --force >/dev/null 2>&1
git branch -q -D "pfs/$ova-ov-a" "pfs/$frid-ov-from" 2>/dev/null

section "venvs"
check "the fork's pip points at the fork's venv" contains "$(head -1 "$forks/2/.venv/bin/pip")" "$forks/2/.venv/"
check "activate points at the fork's venv" grep -q "$forks/2/.venv" "$forks/2/.venv/bin/activate"
check "main's venv is untouched" denied grep -rqF "$forks" .venv/bin
check "python in the fork uses the fork's venv" test "$("$PFS" run 2 .venv/bin/python -c 'import sys; print(sys.prefix)')" = "$forks/2/.venv"

section "forks live outside the repo, one folder per main"
repo "$mains/twin/toy"
twin=$(cd "$mains/twin/toy" && "$PFS" new twin -q)
twin_forks=$(cat "$mains/twin/toy/.git/pfs/forks-dir")
check "a second main named toy gets toy-2" test "$twin_forks" = "$(real "$PFS_HOME")/forks/toy-2"
check "  …its fork lives there" test -d "$twin_forks/$twin"
check "  …its pfs ls sees only its own forks" test "$(cd "$mains/twin/toy" && "$PFS" ls | wc -l | tr -d ' ')" -eq "$(numbered "$twin_forks")"
check "  …and the toy's sees only the toy's" test "$("$PFS" ls | wc -l | tr -d ' ')" -eq "$(numbered "$forks")"
check "  …the toy's .main still names the toy" test "$(cat "$forks/.main")" = "$(real "$toy")"

printf '[forks]\ndir = "%s"\n' "$mains/elsewhere" >"$PFS_HOME/config.toml"
repo "$mains/third"
(cd "$mains/third" && "$PFS" new t -q >/dev/null)
check "[forks] dir sends a new main's forks there" test -d "$mains/elsewhere/third/1"
check "  …in 0700 folders" test "$(stat -f %Lp "$mains/elsewhere" "$mains/elsewhere/third" | sort -u)" = 700
before=$(numbered "$forks")
kept=$("$PFS" new kept-home -q)
check "  …while a main with forks keeps its recorded folder" test -d "$forks/$kept" -a "$(numbered "$forks")" -eq $((before + 1))
check "  …and its record" test "$(cat .git/pfs/forks-dir)" = "$forks"
"$PFS" rm "$kept" >/dev/null
printf '[forks]\ndir = "forks"\n' >"$PFS_HOME/config.toml"
check "a relative [forks] dir is an error" denied "$PFS" ls
check "  …naming the file" contains "$("$PFS" ls 2>&1)" "[forks] dir in $PFS_HOME/config.toml"
rm "$PFS_HOME/config.toml"

repo "$mains/mover"
moving=$(cd "$mains/mover" && "$PFS" new moving -q)
mover_forks=$(cat "$mains/mover/.git/pfs/forks-dir")
mv "$mains/mover" "$mains/moved"
check "a main moved: its fork doesn't know it yet" denied contains "$(cd "$mover_forks/$moving" && "$PFS" ls 2>&1)" "* $moving-moving"
check "  …the moved main still finds its forks" contains "$(cd "$mains/moved" && "$PFS" ls)" "$moving-moving"
check "  …and its .main names it again" test "$(cat "$mover_forks/.main")" = "$(real "$mains/moved")"
check "  …so the fork knows its main again" contains "$(cd "$mover_forks/$moving" && "$PFS" ls)" "* $moving-moving"
cp -R "$mains/moved" "$mains/copy"
check "a copy of a main doesn't take over its forks" denied contains "$(cd "$mains/copy" && "$PFS" ls)" "$moving-moving"
check "  …and .main still names the original" test "$(cat "$mover_forks/.main")" = "$(real "$mains/moved")"
copied=$(cd "$mains/copy" && "$PFS" new copied -q)
check "  …its first fork gets a folder of its own" test -d "$PFS_HOME/forks/copy/$copied" -a "$(cat "$mains/copy/.git/pfs/forks-dir")" = "$(real "$PFS_HOME")/forks/copy"

repo "$mains/legacy"
mkdir -p "$mains/legacy/.forks/.rm-4"
printf '{"type":"fork","id":6,"name":"old","branch":"pfs/6-old","ts":"2026-01-01T00:00:00.000Z"}\n{"type":"rm","id":6,"ts":"2026-01-01T00:01:00.000Z"}\n' >"$mains/legacy/.forks/ledger.jsonl"
touch "$mains/legacy/.forks/.lock"
last=$(jq -r 'select(.type=="fork") | .id' "$mains/legacy/.forks/ledger.jsonl" | sort -n | tail -1)
check "an old .forks/ledger.jsonl is read" contains "$(cd "$mains/legacy" && "$PFS" ls --all)" "6-old   removed"
check "  …from .git/pfs, where it moved" grep -q '"id":6' "$mains/legacy/.git/pfs/ledger.jsonl"
check "  …and the old .forks is gone" test ! -e "$mains/legacy/.forks"
check "  …the next id continues from it" contains "$(cd "$mains/legacy" && "$PFS" new next)" "created $((last + 1))-next"

repo "$mains/old"
mkdir -p "$mains/old/.forks/3"
out=$(cd "$mains/old" && "$PFS" ls 2>&1)
check "a leftover .forks/<n> refuses" denied sh -c "cd '$mains/old' && '$PFS' ls"
check "  …naming it and saying to land it with pfs 0.3" contains "$out" "has forks from pfs 0.3 (3)"
check "  …with pfs rm" contains "$out" "(\`pfs rm 3\` from"
check "  …and leaves it be" test -d "$mains/old/.forks/3"

repo "$mains/homeless"
(cd "$mains/homeless" && env -u PFS_HOME HOME="$mains/home" "$PFS" new h >/dev/null 2>&1)
check "with PFS_HOME unset, forks land in \$HOME/.pfs/forks" test -d "$mains/home/.pfs/forks/homeless/1"
check "  …and the config comes from \$HOME/.pfs too" test "$(cd "$mains/homeless" && mkdir -p "$mains/home/.pfs" && printf '[ports]\nbase = 42000\n' >"$mains/home/.pfs/config.toml" && env -u PFS_HOME HOME="$mains/home" "$PFS" port 1)" = 42010

section "pfs ls outside any repo: every repo's forks"
outside=$(mktemp -d /tmp/pfs-hammer-outside.XXXXXX)
ev_home="$mains/ev-home"
ev() { (cd "$1" && shift && PFS_HOME="$ev_home" "$PFS" "$@"); }
check "no forks dir yet: says so" contains "$(ev "$outside" ls)" "no forks in"
repo "$mains/ev-here"
repo "$mains/ev-gone"
ev "$mains/ev-here" new a -q >/dev/null && ev "$mains/ev-here" new b -q >/dev/null
gone_fork=$(ev "$mains/ev-gone" new wip -q)
gone_dir=$(ev "$mains/ev-gone" path "$gone_fork")
(cd "$gone_dir" && echo x >x && git add -A && git -c user.name=t -c user.email=t@t commit -qm work && echo y >y)
out=$(ev "$outside" ls)
check "a main with forks: one line, its forks counted" grep -qE "^ev-here +.*/ev-here +2 forks$" <<<"$out"
check "  …and nothing more about it" test "$(grep -c 'ev-here' <<<"$out")" -eq 1
mv "$mains/ev-gone" "$mains/ev-moved"
out=$(ev "$outside" ls)
check "a main that's gone: flagged" contains "$out" "its folder is gone: it may have moved"
check "  …with its fork's work" contains "$out" "$gone_fork-wip (1 commit · uncommitted)"
check "  …and how to keep it" contains "$out" "push <remote or repo> pfs/$gone_fork-wip"
ev "$mains/ev-moved" ls >/dev/null
check "  …until pfs runs in the moved main: here again" denied contains "$(ev "$outside" ls)" "its folder is gone"
mv "$mains/ev-moved" "$mains/ev-away" && mkdir "$mains/ev-moved"
check "a plain folder where the main was: replaced" contains "$(ev "$outside" ls)" "something else is there, not a repo"
ev "$mains/ev-here" rm 1 2 >/dev/null
check "a main whose forks are all removed drops off" denied contains "$(ev "$outside" ls)" "ev-here"
mkdir -p "$ev_home/forks/ev-here/.rm-9"
check "  …and a delete in progress isn't a fork" denied contains "$(ev "$outside" ls)" "ev-here"
check "--json outside a repo: refused" denied ev "$outside" ls --json
rm -rf "$outside"

section "nested repos and pipes"
inner="$forks/3/inner"
git init -q "$inner"
git -C "$inner" -c user.name=t -c user.email=t@t commit -q --allow-empty -m inner
check "a repo inside a fork is a main folder of its own" sh -c "cd '$inner' && '$PFS' new deep"
inner_forks=$(cat "$inner/.git/pfs/forks-dir")
check "  …its forks get a folder of their own in the forks dir" test -d "$inner_forks/1" -a "$(dirname "$inner_forks")" = "$(dirname "$forks")"
check "  …while the fork itself still can't fork" denied sh -c "cd '$forks/3' && '$PFS' new nope"
check "  …and from inside fork 3, new/diff/rm work on it" "$PFS" run 3 sh -c "cd inner && '$PFS' new deep2 && '$PFS' diff 2 && '$PFS' rm 2"
check "  …its ledger lives in its own .git/pfs" grep -q '"type":"rm","id":2' "$inner/.git/pfs/ledger.jsonl"
(cd "$inner" && "$PFS" rm 1 --force >/dev/null)
rm -rf "$inner"
check "a closed pipe ends pfs quietly" test -z "$("$PFS" ls 2>&1 >/dev/null | head -1)"

section "pfs new: who's writing in the folder it copies"
cd "$toy" || exit 1
made=()
mkdir -p node_modules/.cache
(exec 3>>node_modules/.cache/db.sqlite 4<>node_modules/.cache/db.sqlite-wal && exec sleep 30) &
wpid=$!
sleep 0.3
out=$("$PFS" new writer 2>&1)
id=$(new_id "$out")
made+=("$id")
check "a writer in main: new still makes the fork" test -n "$id" -a -d "$forks/$id"
check "  …and names the writer, its pid and both files" contains "$out" "note: sleep (pid $wpid) has node_modules/.cache/db.sqlite, node_modules/.cache/db.sqlite-wal open for writing in main, so the fork's copy may be torn"
id=$("$PFS" new writer-q -q 2>"$work/scratch/new-q.err")
made+=("$id")
check "  …with -q, stdout is just the number" grep -qxE '[0-9]+' <<<"$id"
check "  …and the note goes to stderr" grep -q "sleep (pid $wpid) has node_modules/.cache/db.sqlite" "$work/scratch/new-q.err"
out=$("$PFS" new writer-from --from HEAD 2>&1)
made+=("$(new_id "$out")")
check "  …--from HEAD copies main's ignored files too, so it says the same" contains "$out" "sleep (pid $wpid) has node_modules/.cache/db.sqlite"
kill "$wpid" 2>/dev/null
wait "$wpid" 2>/dev/null
(exec 3<node_modules/.cache/db.sqlite && exec sleep 30) &
rpid=$!
(exec 3>>.git/pfs/hammer-writer.log && exec sleep 30) &
opid=$!
sleep 0.3
out=$("$PFS" new quiet-folder 2>&1)
id=$(new_id "$out")
made+=("$id")
check "a reader, and a writer in pfs's own .git/pfs: the fork is made" test -n "$id" -a -d "$forks/$id"
check "  …with no note" denied contains "$out" "open for writing"
kill "$rpid" "$opid" 2>/dev/null
wait "$rpid" "$opid" 2>/dev/null
rm -f .git/pfs/hammer-writer.log
(cd "$forks/$id" && exec 3>>node_modules/.cache/db.sqlite && exec sleep 30) &
fpid=$!
sleep 0.3
out=$("$PFS" new copy-of --of "$id" 2>&1)
made+=("$(new_id "$out")")
check "--of: a writer in that fork's folder is named, in fork $id" contains "$out" "sleep (pid $fpid) has node_modules/.cache/db.sqlite open for writing in fork $id-quiet-folder"
kill "$fpid" 2>/dev/null
wait "$fpid" 2>/dev/null
rm -rf node_modules/.cache
"$PFS" rm "${made[@]}" --force >/dev/null 2>&1
check "  …and every fork this section made is gone" sh -c 'for dir; do test ! -e "$dir" || exit 1; done' _ "${made[@]/#/$forks/}"

section "pfs kill: what's left in a fork, and nothing else"
cd "$toy" || exit 1
id=$("$PFS" new leftovers -q 2>/dev/null)
other=$("$PFS" new bystander -q 2>/dev/null)
lpid=$(cd "$forks/$id" && sh -c 'nohup sleep 30 >/dev/null 2>&1 & echo $!')
bpid=$(cd "$forks/$other" && sh -c 'nohup sleep 30 >/dev/null 2>&1 & echo $!')
mpid=$(sh -c 'nohup sleep 30 >/dev/null 2>&1 & echo $!')
out=$("$PFS" run "$id" --bg sleep 30)
rpid=$(started_pid "$out")
rrun=$(started_run "$out")
sleep 0.3
out=$("$PFS" kill "$id" 2>&1)
code=$?
check "a sub-style orphan in the fork is stopped" contains "$out" "stopped in fork $id-leftovers:"
check "  …named with its pid" contains "$out" "  pid $lpid  sleep"
check "  …gone" denied kill -0 "$lpid"
check "  …and pfs kill exits 0" test "$code" -eq 0
check "the same command in another fork lives on" kill -0 "$bpid"
check "  …and in main" kill -0 "$mpid"
check "a bg run in the fork is left alone" kill -0 "$rpid"
check "  …and listed with rm's hint" contains "$out" "(stop: kill -- -$rpid)"
kill -- -"$rpid" 2>/dev/null
"$PFS" wait "$id.$rrun" >/dev/null 2>&1
kill "$bpid" "$mpid" 2>/dev/null
tpid=$(cd "$forks/$id" && sh -c 'trap "" TERM; nohup sleep 30 >/dev/null 2>&1 & echo $!')
out=$("$PFS" kill "$id" 2>&1)
code=$?
check "an orphan that ignores TERM: pfs kill exits 1" test "$code" -eq 1
check "  …and says what to do next" contains "$out" "pid $tpid  sleep  (kill -9 $tpid)"
kill -9 "$tpid" 2>/dev/null
run=$("$PFS" run "$id" --bg -q sh -c "sh -c 'nohup sleep 30 >/dev/null 2>&1 &'; sleep 0.3; '$PFS' kill")
check "from inside the fork, no number: it ends 0, its own run untouched" "$PFS" wait "$run"
check "  …the orphan it left is stopped" contains "$("$PFS" logs "$run")" "stopped in fork $id-leftovers:"
check "  …and its own run isn't listed" denied contains "$("$PFS" logs "$run")" "pfs runs left alone"
check "in main with no number: refused" contains "$("$PFS" kill 2>&1)" "which fork?"
check "  …and a fork that isn't there: refused" denied "$PFS" kill 999
check "once nothing's left, rm goes through" "$PFS" rm "$id" --force
"$PFS" rm "$other" --force >/dev/null 2>&1

printf '\n%d passed, %d failed\n' "$passed" "$failed"
[ "$failed" -eq 0 ]
