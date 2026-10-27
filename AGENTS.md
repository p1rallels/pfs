# pfs-cli

pfs: forks of a folder, for you and your agents. rust, macOS only (apfs clones). the why behind every decision: DESIGN.md.

## commands

- build: `cargo build`
- format: `cargo fmt` (rustfmt.toml: max_width 120). run it before every commit, so parallel forks don't fight over layout
- lint: `cargo clippy --all-targets` (keep it at zero warnings)
- unit tests: `cargo test`
- end to end: `bash sandbox/hammer.sh` (builds pfs and drives it in a throwaway toy repo under sandbox/work; works from a normal shell and from inside a fork)
- install: `cargo install --path . --force`

## style

- functional: free functions, small pure helpers, iterator pipelines over loops, `match` over if/else chains, no stateful objects
- errors: `anyhow` with context. user-facing messages are lowercase, plain, and say what to do next
- no comments that narrate changes. doc comments only where the why isn't obvious from the code
- tests live next to the code (`#[cfg(test)] mod tests`)
- harness stuff (pi, and any other agent program) lives only in `src/harness.rs`: the env var naming its conversation, and how to read its conversations. pfs never starts an agent: it's a `pfs run --bg` like anything else

## working in a pfs fork

- write only inside your fork. reads are open: other forks and the main folder are someone else's work in progress, so read them, never write there. `pfs ls` says which fork you're in (the `*`)
- commit on your branch (`pfs/<n>-…`) when `cargo fmt`, `cargo test` and `cargo clippy --all-targets` are clean. don't switch branches
- finish clean: everything worth keeping is committed before your final message. `pfs rm` lands commits and nothing else
- never delete a fork's folder by hand, yours or anyone's: `pfs rm` from the main folder is the only way out
- don't push: a fork has your credentials and the network, but the main folder reviews, merges and pushes
- servers listen on `$PORT` (or `pfs port`), never a fixed port like 3000: parallel forks would collide. your block is the 10 ports from `$PORT`
- forks are flat: `pfs new` fails in here. if the task needs more hands, say so in your final message
- you may have a parent: the session that started you. blocked on a decision → `msg_send` to `"parent"` with `wait_seconds`. never message other forks' agents, never acknowledge. your final message is your report: your parent gets it when you end
- `cargo test`, `cargo clippy` and the hammer all work in here. to try your build by hand, make a throwaway repo under `/tmp`, `export PFS_HOME=$(mktemp -d)`, and drive it with `./target/debug/pfs`. never run your build in a real repo (this one's main folder, your own fork): the installed pfs manages those, and a newer build may rewrite their state
- don't edit DESIGN.md, README.md or sandbox/hammer.sh unless the task says so: the main folder writes docs and end-to-end checks at fan-in, so parallel forks don't collide there. say in your final message what end-to-end checks to add
- don't touch `.marbles/`
