# scrumforge

An AI scrum team orchestrator. You act as **product owner**; AI agents — driven via
[`opencode`](https://opencode.ai) — play **scrum master**, **developer**, and
**code reviewer**. Tasks flow across a scrum board, developers work in isolated git
worktrees and open pull requests, reviewers approve+merge or send work back.

Works with any coding agent as the driver: Claude Code, Codex, opencode, Cursor,
etc. — see [AGENTS-CLI.md](AGENTS-CLI.md) for the agent-facing CLI guide.

```text
backlog ─> assigned ─> in-progress ─> in-review ─> done
                          ^               │
                          └─ changes-requested
```

## Requirements

- Rust (to build), [opencode](https://opencode.ai) CLI (runs the agents)
- `git`, and `gh` authenticated (`gh auth login`) for PR flow
- A GitHub project repo you can push branches to

## Install

```bash
./install.sh
```

Builds the release binary, symlinks it into `~/.local/bin`, and adds a `scrumforge`
alias to `~/.bashrc` and `~/.zshrc`.

## Usage

Run from anywhere inside a git repository — the repo root is found automatically and
its board (`.scrumforge.db`, SQLite) is used, so every project keeps its own tasks.

```bash
scrumforge            # interactive TUI (humans)
scrumforge repl       # line-oriented REPL
scrumforge --help     # full usage for every mode
```

### Non-interactive CLI (scripts and AI agents)

Full guide for coding agents (Claude Code, Codex, opencode, …):
[AGENTS-CLI.md](AGENTS-CLI.md).

```bash
scrumforge request "add pagination to the user list endpoint"
scrumforge backlog "fix login bug" | "session expires immediately"
scrumforge tasks              # list the board
scrumforge show 1             # full task detail
scrumforge assign 1 developer # set assignee (backlog -> assigned)
scrumforge run 1              # developer implements, pushes, opens PR
scrumforge run 1              # reviewer approves+merges, or requests changes
scrumforge rework 1           # developer addresses review feedback
scrumforge review 1 "needs tests"  # send back in-progress with feedback
```

`request` asks the scrum master agent to split your request into tasks and assign
each to the developer or reviewer. `run` dispatches a task to its assignee; running
an in-review task triggers the reviewer.

### TUI keys (humans)

| Key | Action |
| --- | --- |
| `hjkl` / arrows | navigate columns and tasks |
| `Enter` | open selected task modal (`r` run, `w` rework, `v` review, `Esc` close) |
| `r` | send selected task to its assignee |
| `w` | developer reworks after changes requested |
| `v` | review: send task back in progress with feedback |
| `R` | ask the scrum master to plan a request |
| `a` | add a backlog task |
| `:` | command mode (`run 3`, `assign 3 reviewer`, `review 3 ...`, `quit`) |
| `?` | help overlay |
| `q` | quit (`Esc` closes modals/help) |

## How it works

- Each repo gets a SQLite board at its root: `.scrumforge.db`.
- Agents are non-interactive `opencode run` invocations that must reply with a
  single JSON object — see [GUIDE.md](GUIDE.md) for the role playbooks and reply
  contracts.
- The developer agent works in a worktree at
  `~/dev/worktrees/<project>/<branch>` (never inside the repo tree), commits with
  Conventional Commits, and scrumforge pushes and opens the PR via `gh`.
- The reviewer reads the PR diff; on approve it squash-merges and removes the
  worktree, on changes it sends the task back with notes.

## Development

```bash
cargo build            # compile
cargo clippy           # lint (kept warning-free)
cargo run              # TUI (from a git repo)
cargo run -- repl      # REPL
cargo run -- tasks     # CLI
```

- [AGENTS.md](AGENTS.md) — guidance for coding agents working on this repo
- [AGENTS-CLI.md](AGENTS-CLI.md) — CLI guide for coding agents *using* scrumforge
- [GUIDE.md](GUIDE.md) — role playbooks for the agents scrumforge hires
- [install.sh](install.sh) — build + alias installer

## Caveats

- The reviewer merges automatically on approve — no human confirmation gate yet.
- Base branch is hard-coded to `main`.
- Agent runs can take minutes; in the TUI a busy overlay shows while they work
  (Ctrl-C aborts, board state is saved per step).
