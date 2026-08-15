# scrumforge

An AI scrum team orchestrator. You act as **product owner**; AI agents — driven via
[`opencode`](https://opencode.ai) — play **scrum master**, **developer**, and
**code reviewer**. Tasks flow across a scrum board, developers work in isolated git
worktrees and open pull requests, reviewers approve+merge or send work back.

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

```bash
scrumforge request "add pagination to the user list endpoint"
scrumforge backlog "fix login bug" | "session expires immediately"
scrumforge tasks              # list the board
scrumforge show 1             # full task detail
scrumforge run 1              # developer implements, pushes, opens PR
scrumforge run 1              # reviewer approves+merges, or requests changes
scrumforge rework 1           # developer addresses review feedback
```

`request` asks the scrum master agent to split your request into tasks and assign
each to the developer or reviewer. `run` dispatches a task to its assignee; running
an in-review task triggers the reviewer.

### TUI keys

| Key | Action |
| --- | --- |
| `hjkl` / arrows | navigate columns and tasks |
| `r` / `Enter` | send selected task to its assignee |
| `w` | developer reworks after changes requested |
| `R` | ask the scrum master to plan a request |
| `a` | add a backlog task |
| `:` | command mode (`run 3`, `rework 3`, `quit`) |
| `?` | help overlay |
| `q` / `Esc` | quit |

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
- [GUIDE.md](GUIDE.md) — role playbooks for the agents scrumforge hires
- [install.sh](install.sh) — build + alias installer

## Caveats

- The reviewer merges automatically on approve — no human confirmation gate yet.
- Base branch is hard-coded to `main`.
- Agent runs can take minutes; in the TUI a busy overlay shows while they work
  (Ctrl-C aborts, board state is saved per step).
