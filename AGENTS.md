# AGENTS.md

Guidance for AI coding agents (opencode, Claude Code, etc.) working in this repository.

## Project

`scrumforge` is a Rust CLI that orchestrates an AI scrum team across multiple
repositories. The human user acts as **product owner**; agent roles are executed
via `opencode run`:

| Role | Methodology | Responsibility |
| --- | --- | --- |
| Scrum master | Scrum | Breaks a PO request into backlog tasks, assigns developer or reviewer |
| Spec planner | SDD | Drafts specs, plans, requirements, and linked tasks |
| Developer | Both | Creates a worktree, implements, commits, pushes, opens a PR via `gh` |
| Code reviewer | Both | Reviews the PR diff, approves + merges, or requests changes |

All projects share one global SQLite store; there is no repo-local board file.
Work is sprint-scoped, with unscheduled tasks in a product backlog. Each project
has a required URL-safe route, a methodology (`scrum` or `sdd`), and a worktree
root. See `docs/design-web-and-multi-project.md` for the authoritative design.

`GUIDE.md` documents the role playbooks and JSON reply contracts that scrumforge
expects from the agents it drives — keep prompts in `src/agents.rs` in sync with it.

## Commands

```bash
cargo build    # compile
cargo clippy   # lint — must be warning-free before finishing any task
cargo run      # starts the TUI
cargo run -- repl   # line-oriented REPL
cargo run -- serve  # web UI and API on loopback
```

There is no test suite yet. Verify changes with `cargo build && cargo clippy` and,
if safe, a smoke test of the CLI, e.g. `cargo run -- --help` or
`printf 'help\nquit\n' | cargo run -- repl --project <route>`. Don't launch the TUI
in agent sessions; it needs an interactive terminal.

## Architecture

- `src/main.rs` — startup: opens the global store and dispatches to `tui`
  (default), `repl`, `serve`/`--web`, or the CLI. `serve` does not require a
  resolved project; it opens the global dashboard.
- `src/board.rs` — global SQLite schema, migrations, models, and `Store` methods
  for projects, sprints, tasks, specs, changes, events, counts, and atomic task
  claims. Status values: `backlog`, `todo`, `in-progress`, `in-review`,
  `changes-requested`, `done`, `cancelled`.
- `src/resolve.rs` — default DB/config paths, `.scrumforge.toml` binding, route
  validation, and project/profile resolution.
- `src/ops.rs` — shared actions (`request`, `run_task`, `rework`,
  `add_backlog_task`, move/assign, sprints, SDD spec/change) used by all
  frontends. Returns human-readable result strings.
- `src/cli.rs` — global flag parsing plus project, sprint, backlog, task, spec,
  change, context, activity, worktree, and db commands.
- `src/repl.rs` — line-oriented REPL (rustyline) with project/sprint context.
- `src/serve.rs` — axum HTTP frontend (`--web`/`serve`, loopback only). Serves an
  embedded SPA and a project-scoped JSON API under `/api/projects`.
- `src/web/index.html` — self-contained dashboard and project views: overview,
  sprint board, backlog, sprints, SDD specs, and activity.
- `src/tui.rs` — interactive terminal UI (ratatui + crossterm): board/backlog
  views, sprint cycling, detail pane, vim-style keys (`hjkl`/arrows, `r` run,
  `w` rework, `v` review, `R` request, `a` add, `b` view, `s` sprint, `:` command
  mode, `?` help). Long agent runs are queued as `Action`s and executed after the
  busy frame is drawn (`pending` field).
- `src/agents.rs` — all agent interaction and git/gh shell-outs:
  - `ask_agent(role, workdir, prompt, project, actor)` runs `opencode run`
    non-interactively, injects `SCRUMFORGE_PROJECT`/`SCRUMFORGE_PROFILE`, and
    expects a JSON object back; `parse_json` extracts the first `{...}` block.
  - Worktrees live at `<worktree_root>/<route>/<branch>`, defaulting to
    `~/dev/worktrees/<route>/<branch>` — **never** inside the repo tree (user
    policy, see global CLAUDE.md). Branches are `scrumforge/task-<id>`.
  - `developer_work` creates the worktree from the project's base branch, lets
    the agent commit, then pushes and opens the PR. `reviewer_work` reads the diff
    via `gh pr diff`, and on approve runs
    `gh pr merge --squash --delete-branch` and removes the worktree.
    `developer_rework` rebases, fixes, pushes.

## Conventions

- No comments in code unless asked.
- Conventional Commits for any commit made in this repo
  (subject < 72 chars, imperative, lowercase after colon).
- Keep dependency additions minimal; current deps: `anyhow`, `axum`, `crossterm`,
  `ratatui`, `rusqlite` (bundled), `rustyline`, `serde`, `serde_json`, `tokio`.
- Web UI edits stay in `src/web/index.html` (inline CSS/JS, no build step).

## Known gaps (good candidates if asked to improve)

- Reviewer auto-merges on approve with no human confirmation gate.
- No retry if an agent returns unparseable output (`parse_json` fails hard).
- The HTTP API is a subset of the design spec: no project PATCH/DELETE, no
  task PATCH, no rework/review endpoints, and no SSE progress feed yet.
- The backlog supports moving tasks into a sprint, but not drag-and-drop
  reordering.
- Agent prompts ask for JSON but models sometimes wrap in fences; `parse_json`
  tolerates leading/trailing text but not nested fences.
