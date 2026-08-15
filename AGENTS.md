# AGENTS.md

Guidance for AI coding agents (opencode, Claude Code, etc.) working in this repository.

## Project

`scrumforge` is a Rust CLI that orchestrates an AI scrum team. The human user acts as
**product owner** in a REPL; three agent roles are executed via `opencode run`:

| Role | Responsibility |
| --- | --- |
| Scrum master | Breaks a PO request into tasks, assigns each to developer or reviewer |
| Developer | Creates a git worktree, implements, commits, pushes, opens a PR via `gh` |
| Code reviewer | Reviews the PR diff, approves + merges, or requests changes |

State is a JSON board persisted at `.scrumforge.json` in the target repo root.

`GUIDE.md` documents the role playbooks and JSON reply contracts that scrumforge
expects from the agents it drives — keep prompts in `src/agents.rs` in sync with it.

## Commands

```bash
cargo build    # compile
cargo clippy   # lint — must be warning-free before finishing any task
cargo run      # starts the TUI (must be run from a git repo root)
cargo run -- repl  # line-oriented REPL instead of TUI
```

There is no test suite yet. Verify changes with `cargo build && cargo clippy` and, if
safe, a smoke test of the REPL, e.g. `printf 'help\nquit\n' | cargo run -- repl`
(run from a git repo — this repo works). Don't launch the TUI in agent sessions; it
needs an interactive terminal.

## Architecture

- `src/main.rs` — startup: validates cwd is a git repo, loads the board,
  dispatches to `tui` (default) or `repl` subcommand.
- `src/ops.rs` — shared actions (`request`, `run_task`, `rework`,
  `add_backlog_task`) used by both frontends. Returns human-readable result
  strings.
- `src/repl.rs` — line-oriented REPL (rustyline).
- `src/tui.rs` — interactive terminal UI (ratatui + crossterm): five status
  columns, detail pane, vim-style keys (`hjkl`/arrows, `r` run, `w` rework,
  `R` request, `a` add, `:` command mode, `?` help). Long agent runs are queued
  as `Action`s and executed after the busy frame is drawn (`pending` field).
- `src/board.rs` — `Task`, `Status`, `Board` types; JSON load/save. Status flow:
  `Backlog → Assigned → InProgress → InReview → (ChangesRequested → InReview)* → Done`.
- `src/agents.rs` — all agent interaction and git/gh shell-outs:
  - `ask_agent(role, workdir, prompt)` runs `opencode run` non-interactively and
    expects a JSON object back; `parse_json` extracts the first `{...}` block.
  - Worktrees live at `~/dev/worktrees/<repo-name>/<branch>` — **never** inside the
    repo tree (user policy, see global CLAUDE.md). Branches are `scrumforge/task-<id>`.
  - `developer_work` creates the worktree (base `main`), lets the agent commit,
    then pushes and opens the PR. `reviewer_work` reads the diff via
    `gh pr diff`, and on approve runs `gh pr merge --squash --delete-branch` and
    removes the worktree. `developer_rework` rebases, fixes, pushes.

## Conventions

- No comments in code unless asked.
- Conventional Commits for any commit made in this repo
  (subject < 72 chars, imperative, lowercase after colon).
- Keep dependency additions minimal; current deps: `serde`, `serde_json`, `anyhow`,
  `rustyline`, `ratatui`, `crossterm`.

## Known gaps (good candidates if asked to improve)

- Reviewer auto-merges on approve with no human confirmation gate.
- No retry if an agent returns unparseable output (`parse_json` fails hard).
- Base branch is hard-coded to `main`.
- Agent prompts ask for JSON but models sometimes wrap in fences; `parse_json`
  tolerates leading/trailing text but not nested fences.
