#!/bin/sh
# Stand-in for a headless agent, started the way a real sub is:
#   pfs run <n> --bg sandbox/fake-agent.sh -p "<task>"
# Works in its folder and commits, like a small, well-behaved agent. With
# "converse" in its task it also keeps a pi-style conversation in
# PI_CODING_AGENT_SESSION_DIR, mid-turn (running tools) while it works,
# closed when it's done.

task=""
while [ $# -gt 0 ]; do
  case "$1" in
    -p) ;;
    *) task="$task${task:+ }$1" ;;
  esac
  shift
done

echo "fake-agent port=${PORT:-unset}"
echo "cwd=$(pwd)"
echo "task=$task"

conversation=""
case "$task" in
  *converse*)
    if [ -n "$PI_CODING_AGENT_SESSION_DIR" ]; then
      mkdir -p "$PI_CODING_AGENT_SESSION_DIR"
      conversation="$PI_CODING_AGENT_SESSION_DIR/fake-$$.jsonl"
      printf '{"type":"session","version":3,"id":"fake-%s","timestamp":"2026-01-01T00:00:00.000Z","cwd":"%s"}\n' "$$" "$(pwd -P)" >"$conversation"
    fi
    ;;
esac
say() { [ -z "$conversation" ] || printf '{"type":"message","message":%s}\n' "$1" >>"$conversation"; }

say '{"role":"user","content":"task"}'
say '{"role":"assistant","content":[],"stopReason":"toolUse"}'
case "$task" in
  *sleep*) sleep "$(echo "$task" | sed -n 's/.*sleep \([0-9]*\).*/\1/p')" ;;
esac
say '{"role":"toolResult","content":[]}'

echo "note from the fake agent: $task" > agent-note.txt
git add agent-note.txt && git commit -qm "fake agent: $task" && echo "committed"
say '{"role":"assistant","content":[],"stopReason":"stop"}'
