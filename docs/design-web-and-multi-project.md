# Design: single-store multi-project, Scrum, SDD, and web view

Status: proposed (revised)
Date: 2026-10-16, revised 2026-10-26
Related: `README.md`, `AGENTS.md`, `AGENTS-CLI.md`, `GUIDE.md`

> [!note]
> This revision replaces the earlier per-project board decision. Boards are no
> longer stored in each repository. One central database owns every project,
> sprint, spec, task, and event. It also adds Scrum sprints and Spec-Driven
> Development (SDD) as first-class methodologies.

## Context

`scrumforge` is a single-binary Rust orchestrator for an AI scrum team. The human
acts as product owner; scrum master, developer, and reviewer roles are executed via
`opencode run`. Today it has three frontends sharing one action layer:

- `src/cli.rs`, `src/repl.rs`, `src/tui.rs` — dispatch through `src/ops.rs`.
- `src/board.rs` — one SQLite board per repo at `.scrumforge.db`, located by
  walking up from the cwd to the git root (`find_repo_root`, `src/main.rs:13`).
- `src/agents.rs` — `ask_agent` shells out to `opencode run` and blocks until the
  process exits; a run can take minutes.

The new requirements are structural, not cosmetic:

1. **One database for all projects.** A project is a row in a global store, not a
   file inside a repository.
2. **Projects are addressed by a required route.** Every project has a stable,
   unique route used by the CLI, the HTTP API, the web UI, and agent prompts.
3. **Every command can target a project explicitly.** Commands work from any
   directory and accept `--project <route>`; agents never have to guess.
4. **Many worktrees, one project.** Developers can work concurrently in separate
   git worktrees while all state updates land in the same project record.
5. **Scrum methodology.** Product backlog, sprints, and a board per sprint rather
   than one board per project, with the current card/column design retained.
6. **SDD methodology.** Specs, plans, requirements, and change proposals are
   first-class artifacts, with tasks traced back to approved specs.
7. **Jira-like navigation, current visual design.** Projects, backlog, sprints,
   and specs are browsable views; the existing Trello-style board remains the
   board view for a single sprint.

## Goals and non-goals

### Goals

- One local SQLite database at a well-known path, shared by every frontend and
  every repository managed by this install.
- A required, unique, URL-safe project `route` and an explicit project resolver.
- Global CLI flags (`--project`, `--profile`, `--json`, `--db`) accepted by every
  project-scoped command.
- Scrum: product backlog, timeboxed sprints, sprint planning, sprint board,
  sprint review/retro notes, and sprint close handling.
- SDD: constitution, specifications, requirements, plans, tasks, change
  proposals, verification, and traceability to tasks.
- A web UI with a global project dashboard and per-project `/p/:route/...`
  views, plus a TUI that keeps its current look with project and sprint context.
- Safe concurrent agent runs: atomic task claim, separate worktrees, PR-based
  integration, and a per-project merge lock.

### Non-goals

- Multi-tenant authentication, remote users, or exposing `serve` beyond loopback.
- PostgreSQL or a separate control-plane process; this remains local-first.
- A plugin system for arbitrary methodologies.
- Per-project custom board columns in v1.
- Replacing GitHub PR review with an internal review engine.
- Full Jira parity (issue keys, custom fields, workflows, filters, dashboards).

## Summary of decisions

| Area | Decision |
| --- | --- |
| Storage | One global SQLite DB at `~/.local/share/scrumforge/scrumforge.db` |
| Project identity | Required `route`, unique and URL-safe, e.g. `scrumforge` |
| Command targeting | `--project <route>`, then env, then `.scrumforge.toml`, then cwd |
| Worktrees | Many per project under `~/dev/worktrees/<route>/<branch>` |
| Methodology | `projects.methodology` is `scrum` or `sdd` |
| Execution unit | A sprint for both methodologies; a board is always sprint-scoped |
| Backlog | Tasks with no sprint; separate view, not a board column |
| Scrum | Product backlog → sprint planning → sprint board → review → close |
| SDD | Spec → plan → tasks → implement → verify → archive, linked into sprints |
| Web | Global dashboard plus `/p/:route/...` project routes |
| API | `/api/projects/...`; SSE for live per-project events |
| Identity | Profiles and an append-only event log, as in the previous revision |

## Architecture

```text
scrumforge
├── core/                 Store, migrations, resolver, projects, sprints, specs
├── cli | repl | tui      Shared command/action layer over core
└── serve                 axum HTTP server + embedded web UI
     ├── GET  /                                  global project dashboard
     ├── GET  /p/:route/...                      project views
     ├── GET  /api/projects                      project registry
     ├── GET  /api/projects/:route/tasks         project task list
     ├── GET  /api/projects/:route/sprints       sprint list
     ├── GET  /api/projects/:route/events        SSE live progress
     └── GET  /api/events                        global SSE feed
```

All frontends share one `Store`. No frontend owns a board file, and no command
opens a repository-local database. The store is the source of truth for projects,
sprints, tasks, specs, events, and agent runs.

## Single global store

### Location and configuration

- Default database: `~/.local/share/scrumforge/scrumforge.db`
  (`$XDG_DATA_HOME/scrumforge/scrumforge.db` when set).
- Overrides, highest priority first:
  1. `--db <path>` on the command line.
  2. `SCRUMFORGE_DB` environment variable.
  3. `database.path` in `~/.config/scrumforge/config.toml`.
  4. The XDG default above.
- The DB is created with `PRAGMA journal_mode=WAL`, `foreign_keys=ON`, and a
  `busy_timeout` so the CLI, TUI, and `serve` can coexist.
- Schema changes are versioned migrations run at startup. A newer binary refuses
  to open an older schema until migrations complete successfully.
- No repository receives a `.scrumforge.db` after cutover. A legacy file is only
  read by the explicit migration command.

### Global configuration

`~/.config/scrumforge/config.toml` holds install-wide defaults only. Projects
live in the database; duplicating them in TOML would let the two drift.

```toml
[account]
username = "manaseslovera"

[defaults]
worktree_root = "~/dev/worktrees"
max_parallel_runs = 2
default_profile = "manaseslovera"

[[profiles]]
id = "dev-1"
kind = "agent"
roles = ["developer"]
cli = "opencode"
```

Project-specific settings (route, repo path, default branch, methodology,
concurrency) belong in the `projects` table. A repository-local
`.scrumforge.toml` is only a binding, not a second source of truth.

### Process and concurrency model

- Writes use short transactions; task claim uses `BEGIN IMMEDIATE` plus a
  conditional `UPDATE` so two agents cannot claim the same task.
- Claims carry `claimed_by`, `claimed_at`, and `lease_until`. A stale lease can be
  reclaimed after expiry.
- The in-process web server shares one `Store` behind a lock or pool. Separate
  CLI processes rely on SQLite WAL and `busy_timeout`.
- Long agent runs happen outside a DB transaction. State transitions are
  recorded before and after the run.
- A per-project merge lock serializes PR merges, not development runs.
- `scrumforge db backup [--out <path>]` and `scrumforge db check` are available
  for operational safety.

## Projects and routes

### Route definition

A **route** is the project's stable public identifier:

- Required at project creation; there is no null or empty route.
- Lowercase ASCII letters, digits, and `-`; must start and end with a letter or
  digit; length 2–48; pattern `^[a-z0-9](?:[a-z0-9-]*[a-z0-9])$`.
- Unique case-insensitively across the whole database.
- Reserved and rejected: `api`, `assets`, `static`, `p`, `admin`, `health`,
  `events`, `projects`, `new`, and names beginning with `_`.
- Immutable after creation. Renaming requires an explicit
  `scrumforge project rename-route` that also reports stale `.scrumforge.toml`
  bindings; the old route stops resolving immediately.
- Used verbatim in CLI flags, prompts, environment variables, API paths, and web
  URLs. It is not a security boundary.

### Project record

| Field | Meaning |
| --- | --- |
| `id` | Internal integer key |
| `route` | Required, unique public identifier |
| `name` | Human-readable display name |
| `repo_path` | Canonical absolute path to the main checkout |
| `default_branch` | Integration branch, default `main` |
| `worktree_root` | Root for task worktrees, default `~/dev/worktrees` |
| `methodology` | `scrum` or `sdd` |
| `max_parallel_runs` | Per-project concurrency cap, default 2 |
| `git_remote` | Optional cached remote URL |
| `archived_at` | Null unless the project is archived |
| `created_at`, `updated_at` | Timestamps |

`repo_path` is canonicalized with the filesystem real path. It must be a git
repository. `name` may change freely; `route` may not.

### Registering a project

```bash
scrumforge project add scrumforge ~/dev/scrumforge \
  --name "Scrumforge" \
  --methodology scrum \
  --default-branch main \
  --worktree-root ~/dev/worktrees \
  --max-parallel-runs 2
```

Rules:

- `--route` is required, or the route is the required positional argument.
- Registering the same route twice fails; registering the same `repo_path` twice
  fails unless `--force` is used to replace the binding.
- `project add` rejects a path that is not a git repository.
- If a legacy `.scrumforge.db` exists at that path, `project add` reports it and
  requires `--import` to migrate. Nothing is silently imported.
- `project list` shows route, name, methodology, active sprint, and counts.
- `project show <route>` shows settings and paths.
- `project archive <route>` hides it from default views but preserves history.
- `project rm <route>` requires `--yes` and refuses while an active sprint or
  running task exists.

### Binding a repository to a route

A committed `.scrumforge.toml` at the repository root binds checkouts and
worktrees to the project. Worktrees inherit it because it is tracked.

```toml
# .scrumforge.toml
route = "scrumforge"
default_profile = "dev-1"
```

The repository's `AGENTS.md` or `CLAUDE.md` should state the same binding:

```markdown
## scrumforge

This repository is scrumforge project `scrumforge`.
This agent's profile is `dev-1`.
Always pass `--project scrumforge`.
```

`default_profile` is a convenience for agents; it does not replace the explicit
`--profile` or `SCRUMFORGE_PROFILE` when an action must be attributed precisely.

### Project resolution order

For every project-scoped command, resolve the project in this order:

1. `--project <route>` on the command line.
2. `SCRUMFORGE_PROJECT` environment variable.
3. The nearest `.scrumforge.toml` walking up from the cwd, including worktrees.
4. Registry lookup by cwd path: the cwd is inside `repo_path`, or inside
   `<worktree_root>/<route>/...`.
5. Interactive picker when stdin and stdout are a TTY.

If none match and the session is non-interactive, exit non-zero with a message
that lists known routes and suggests `--project`. Never guess silently. When
several projects could match by path, prefer the deepest `worktree_root`/repo
match, then fail with an ambiguity error.

`scrumforge project current [--json]` prints the resolved project, and
`scrumforge project use <route>` can set a shell-local default through the
documented environment variable. There is no hidden global "current project"
that silently changes other sessions.

## Command targeting and agent use

### Global flags

All project-scoped commands accept these flags. The parser strips them wherever
they appear, before or after the subcommand.

| Flag | Purpose |
| --- | --- |
| `--project <route>` | Target one project explicitly |
| `--profile <id>` | Actor identity for the command |
| `--json` | Machine-readable output for agents and scripts |
| `--db <path>` | Alternative central database |
| `--all` | Aggregate across all projects where the command supports it |

```bash
# Explicit project, from any directory
scrumforge --project scrumforge tasks
scrumforge tasks --project scrumforge
scrumforge --project scrumforge --profile dev-1 run 12

# Aggregate views
scrumforge tasks --all --json
scrumforge sprint list --all --state active

# Resolve from a worktree binding
cd ~/dev/worktrees/scrumforge/scrumforge/task-12
scrumforge tasks
```

### Agent contract

When scrumforge starts an agent it provides:

- `SCRUMFORGE_PROJECT=<route>` and `SCRUMFORGE_PROFILE=<profile>` in the child
  environment.
- The worktree path, base branch, task id, sprint, and (for SDD) spec/change ids
  in the prompt envelope.
- A prompt rule that every scrumforge command must include `--project <route>`
  even when the cwd appears to imply it.

Example developer invocation:

```bash
SCRUMFORGE_PROJECT=scrumforge SCRUMFORGE_PROFILE=dev-1 \
  scrumforge --project scrumforge run 12
```

The agent works only inside its assigned worktree. It must not assume that the
repository name, the directory name, or the cwd is the project identity.

`scrumforge context --project <route> --json` prints the resolved project,
active sprint, methodology, default branch, worktree root, and profile. Agents may
call it once at startup instead of parsing human output.

## Worktrees and concurrent work

A project is one logical board with many possible worktrees:

```text
~/dev/scrumforge                                project repo_path (main checkout)
~/dev/worktrees/scrumforge/scrumforge/task-12   task 12 worktree
~/dev/worktrees/scrumforge/scrumforge/task-18   task 18 worktree
```

- Branch name: `scrumforge/task-<id>`. Task ids are global and unique, so branch
  names do not collide across projects or sprints.
- Worktree path: `<worktree_root>/<route>/<branch>`, e.g.
  `~/dev/worktrees/scrumforge/scrumforge/task-12` (branch
  `scrumforge/task-12`).
- Every task run gets its own worktree. Two tasks in the same project may run
  concurrently, subject to `max_parallel_runs` and the profile queue.
- Before work starts, the task is claimed atomically and marked `in-progress`.
- Before push, the developer rebases onto the project's current
  `default_branch`. If the rebase conflicts, the run fails with a clear message
  and the task returns to `changes-requested`.
- The reviewer merges with `gh pr merge --squash --delete-branch`. Merges for one
  project are serialized by the project merge lock so two PRs do not race the
  base branch.
- On merge, the worktree is removed. `scrumforge worktree list` and
  `scrumforge worktree prune` handle leftovers.

This is what allows several worktrees to update a single project safely: they
share one database and one integration branch, but never share a working tree.

## Methodologies

`projects.methodology` selects the planning model. Both models use the same
sprint execution container, so every board is still a sprint board. The
methodology may be changed only when the project has no active sprint; existing
tasks, specs, and events are retained.

### Scrum methodology

Scrum projects use the following flow:

```text
product backlog
      │  sprint planning
      ▼
sprint (planned) ── activate ──▶ sprint (active)
                                    │
                                    ▼
                    Todo → InProgress → InReview → Done
                                  ▲          │
                                  └─ ChangesRequested
                                    │
                              sprint close
                                    ▼
                          sprint (completed)
```

Rules:

- **Product backlog** is the ordered list of tasks with `sprint_id IS NULL` and
  `status = backlog`.
- **Sprint planning** moves backlog tasks into a planned sprint and sets their
  status to `todo`. Tasks may also be created directly in a sprint during
  planning.
- A sprint has a number, name or goal, start/end dates, capacity, and status
  `planned | active | completed`.
- At most one sprint per project is `active`.
- The sprint board shows only tasks in that sprint. The product backlog is a
  separate view, not a sixth board column.
- `sprint close` requires a decision for unfinished tasks: move them to a target
  sprint or return them to the backlog. Completed tasks stay attached to the
  closed sprint for history.
- Sprint review and retrospective notes are stored on the sprint.
- `request` creates backlog tasks (or planned-sprint tasks when explicitly
  requested) through the scrum master role, as today.

### SDD methodology

SDD means **Spec-Driven Development**: the approved specification, not an
ad-hoc request, is the source of truth for what gets built.

Artifacts:

| Artifact | Purpose |
| --- | --- |
| Constitution | Project principles and non-negotiables, one per project |
| Specification | What and why: requirements, acceptance criteria, non-goals |
| Plan | Technical approach, affected areas, risks, test strategy |
| Tasks | Decomposed work items linked to requirements |
| Change proposal | A scoped delta against an existing spec |
| Verification | Reviewer evidence that acceptance criteria are met |

Lifecycle:

```text
constitution
    │
draft spec ──▶ in-review ──▶ approved ──▶ implementing ──▶ implemented
                                                   │
                                              archived
    │
    └─ change proposal (proposed → approved → implementing → done → archived)
```

Rules:

- `request` on an SDD project creates a **draft spec or change proposal**, not
  directly actionable backlog tasks.
- A human approves the spec (`spec approve`). Approval records the actor and
  freezes the requirements version.
- `spec plan <slug>` asks the spec planner to produce a plan and candidate tasks.
  Each generated task links to one or more requirement ids.
- Approved tasks enter the product backlog and can be pulled into a sprint
  during sprint planning. SDD therefore still has sprints and a sprint board.
- A change proposal is an epic-like grouping. It appears as a filter or swimlane
  on the sprint board, not as a separate board.
- The developer implements one linked task at a time inside a worktree.
- The reviewer verifies both code quality and requirement coverage. A task
  cannot be marked `done` without its acceptance criteria being checked.
- A spec becomes `implemented` when all linked tasks are done and verified; a
  change becomes `done` under the same rule. Archiving is explicit.
- Spec content is canonical in the central database, so it is visible across
  projects and frontends. `spec export` writes `specs/<slug>/spec.md`,
  `plan.md`, `tasks.md`, and (optionally) `constitution.md` into the repository
  for review and PR history. Export is one-way in v1; the DB remains canonical.

### Shared task model across methodologies

Both methodologies use the same `tasks` table. The methodology changes how tasks
are planned and what extra traceability is required, not how they are executed.

- Scrum tasks need a sprint and an assignee before `run`.
- SDD tasks additionally need an approved spec and at least one requirement link
  before `run`.
- Both use the status flow
  `backlog → todo → in-progress → in-review → (changes-requested → in-review)* → done`.
- `cancelled` is an optional terminal state recorded in events; cancelled tasks
  leave the active board and remain queryable in history.

## Data model

The following is the logical schema. Migration SQL may split or rename tables,
but the invariants below are part of the spec.

### Core tables

| Table | Key fields |
| --- | --- |
| `projects` | `id`, `route` (unique), `name`, `repo_path` (unique), `default_branch`, `worktree_root`, `methodology`, `max_parallel_runs`, `git_remote`, `archived_at`, timestamps |
| `profiles` | `id`, `username`, `kind` (`human`/`agent`), `roles`, `cli`, timestamps |
| `sprints` | `id`, `project_id`, `number`, `name`, `goal`, `status`, `starts_at`, `ends_at`, `capacity`, `review_notes`, `retro_notes`, timestamps |
| `tasks` | `id`, `project_id`, `sprint_id`, `parent_id`, `kind`, `title`, `description`, `acceptance_criteria`, `status`, `priority`, `estimate`, `position`, `assignee_profile_id`, `created_by`, `branch`, `pr_url`, `review_notes`, `claimed_by`, `claimed_at`, `lease_until`, timestamps |
| `events` | `id`, `project_id`, `sprint_id`, `task_id`, `actor_profile_id`, `verb`, `from_status`, `to_status`, `detail`, `ts` |
| `comments` | `id`, `task_id`, `actor_profile_id`, `body`, `ts` |

### SDD tables

| Table | Key fields |
| --- | --- |
| `project_documents` | `id`, `project_id`, `kind` (`constitution`), `body_md`, `version`, `updated_by`, timestamps |
| `specs` | `id`, `project_id`, `slug`, `title`, `status`, `current_version`, `created_by`, timestamps |
| `spec_versions` | `id`, `spec_id`, `version`, `summary`, `body_md`, `plan_md`, `tasks_md`, `status`, `approved_by`, `approved_at`, `created_at` |
| `requirements` | `id`, `spec_version_id`, `key`, `kind`, `text`, `priority` |
| `changes` | `id`, `project_id`, `spec_id`, `slug`, `title`, `status`, `body_md`, `created_by`, timestamps |
| `task_links` | `task_id`, `spec_id`, `requirement_id`, `change_id` |

### Invariants

- `projects.route` is `NOT NULL UNIQUE COLLATE NOCASE`. It changes only through
  the explicit `project rename-route` command, never through ordinary project
  updates.
- `projects.repo_path` is unique among non-archived projects.
- `sprints.number` is unique per project and increases monotonically.
- At most one `sprints.status = 'active'` row exists per project. Enforce with a
  partial unique index.
- `tasks.sprint_id IS NULL` implies `tasks.status IN ('backlog', 'cancelled')`.
  A task with a sprint may not be `backlog`.
- `tasks.sprint_id` must reference a sprint in the same project.
- A task's `sprint_id` may only change through sprint planning, sprint close, or
  an explicit move to backlog; each change is recorded as an event.
- `task_links.spec_id` must reference a spec in the same project as the task.
- Every mutation appends an event per affected entity (task, sprint, spec), and
  one command-level event when a command changes several entities. Every event
  carries an actor.
- `specs.slug` is unique per project; `changes.slug` is unique per project.
- Only one active sprint per project may be edited on the board; closed sprint
  boards are read-only except for explicit reopen.
- Deleting a project is a hard delete only after `--yes`; history may be retained
  as an archived project instead.

### Status model

| Status | Meaning | Board column |
| --- | --- | --- |
| `backlog` | Unscheduled product-backlog item | No; backlog view |
| `todo` | Scheduled in a sprint, not started | Todo |
| `in-progress` | Claimed and being implemented | In Progress |
| `in-review` | PR open, awaiting reviewer | In Review |
| `changes-requested` | Review rejected; developer to rework | Changes Requested |
| `done` | Merged or verified complete | Done |
| `cancelled` | Abandoned; retained in history | No |

The allowed transitions remain the current ones:

```text
backlog → todo
todo → backlog | in-progress
in-progress → todo | in-review
in-review → in-progress | changes-requested | done
changes-requested → in-progress | in-review
done → in-review              (reopen for corrections)
any non-done → cancelled
```

## Web UI and routes

The current Trello-style drag-and-drop board (`src/web/index.html`) remains the
board component. The surrounding app becomes a Jira-like shell with a project
switcher and sprint navigation. Visual language, card design, dark styling, and
drag behavior are preserved.

### Page routes

| Route | View |
| --- | --- |
| `/` | Global dashboard: project cards, methodology, active sprint, backlog/done counts, recent activity |
| `/p/:route` | Project overview: active sprint summary, backlog summary, specs (SDD), recent activity |
| `/p/:route/backlog` | Ordered product backlog; drag into a sprint |
| `/p/:route/sprints` | Sprint list: planned, active, completed; create, activate, close |
| `/p/:route/sprints/:number` | Sprint board (the existing board design) |
| `/p/:route/specs` | SDD specs list with status and requirement counts |
| `/p/:route/specs/:slug` | Spec detail: requirements, plan, linked tasks, changes |
| `/p/:route/changes` | SDD change proposals and their status |
| `/p/:route/activity` | Project activity feed, filterable by actor, task, or sprint |

Board behavior:

- The board always represents exactly one sprint. If the project has no active
  sprint, `/p/:route/sprints` is the landing view and the board prompts planning.
- Columns are Todo, In Progress, In Review, Changes Requested, and Done.
- Dragging between columns applies the same transition rules as the CLI. Dragging
  to the backlog view or choosing "move to backlog" clears `sprint_id`.
- The sprint header shows goal, dates, capacity, and a sprint selector.
- SDD projects show change/spec badges on cards and can filter by change.
- Completed sprints render read-only boards with their final task positions.

### API routes

| Method and path | Purpose |
| --- | --- |
| `GET /api/projects` | List projects |
| `POST /api/projects` | Register a project with required route |
| `GET /api/projects/:route` | Project settings and summary |
| `PATCH /api/projects/:route` | Update name, branch, methodology, concurrency |
| `DELETE /api/projects/:route` | Archive or delete with confirmation |
| `GET/POST /api/projects/:route/sprints` | List or create sprints |
| `PATCH /api/projects/:route/sprints/:number` | Activate, close, edit goal/dates |
| `GET /api/projects/:route/sprints/:number/tasks` | Sprint board tasks |
| `GET/POST /api/projects/:route/tasks` | Backlog/task list or create |
| `PATCH /api/projects/:route/tasks/:id` | Move, assign, edit, or reorder |
| `POST /api/projects/:route/tasks/:id/run` | Spawn the assignee worker |
| `POST /api/projects/:route/tasks/:id/rework` | Spawn developer rework |
| `POST /api/projects/:route/tasks/:id/review` | Record review feedback |
| `GET/POST /api/projects/:route/specs` | List or create specs (SDD) |
| `GET/PATCH /api/projects/:route/specs/:slug` | Read or update a spec |
| `POST /api/projects/:route/specs/:slug/approve` | Approve a spec version |
| `POST /api/projects/:route/specs/:slug/plan` | Generate plan and tasks |
| `GET/POST /api/projects/:route/changes` | List or create change proposals |
| `GET /api/projects/:route/events` | SSE stream for project progress |
| `GET /api/events` | SSE stream for all projects |

The old unscoped `/api/tasks` routes may exist only as deprecated aliases that
resolve a project from `.scrumforge.toml`; they are not the primary API.

### TUI

The TUI keeps its current ratatui look and vim-style keys, with a project and
sprint header:

- `P` opens a project picker; `S` opens a sprint picker.
- Views/tabs: Board, Backlog, Sprints, Specs (SDD only), Activity.
- `hjkl`/arrows navigate; `r` run, `w` rework, `v` review, `R` request, `a` add.
- `:` command mode accepts the same project-scoped commands as the CLI.
- `?` shows help drawn from the shared help source.
- Long agent runs remain queued after the busy frame; state is written to the
  central store at each transition.
- If no project is resolved at startup, show the project picker instead of
  failing at the git-root check.

## CLI surface

### Project and sprint commands

```bash
# Projects
scrumforge project add <route> <path> [--name <name>] \
  [--methodology scrum|sdd] [--default-branch main] \
  [--worktree-root <path>] [--max-parallel-runs <n>] [--import]
scrumforge project list [--all] [--json]
scrumforge project show <route> [--json]
scrumforge project set <route> [--name ...] [--path ...] [--methodology ...] \
  [--default-branch ...] [--worktree-root ...] [--max-parallel-runs ...]
scrumforge project archive <route>
scrumforge project rm <route> --yes
scrumforge project rename-route <old> <new>
scrumforge project current [--json]
scrumforge project migrate <path> --route <route>

# Sprints
scrumforge sprint add --name "Sprint 1" --goal "..." [--start YYYY-MM-DD] [--end YYYY-MM-DD] [--capacity <n>]
scrumforge sprint list [--state planned|active|completed] [--json]
scrumforge sprint show <number|current|next> [--json]
scrumforge sprint activate <number>
scrumforge sprint close <number> [--move-unfinished backlog|<number>]
scrumforge sprint board <number|current> [--json]
```

### Work commands

```bash
scrumforge backlog add "<title> | <description>" [--sprint <number>]
scrumforge backlog list [--json]
scrumforge tasks [--sprint current|backlog|all|<number>] [--all] [--json]
scrumforge show <id> [--json]
scrumforge assign <id> <profile|role>
scrumforge move <id> <status>
scrumforge request "<text>" [--sprint <number>]
scrumforge run <id>
scrumforge rework <id>
scrumforge review <id> "<feedback>"
```

`request` is methodology-aware:

- Scrum: the scrum master splits the request into backlog tasks.
- SDD: the spec planner creates a draft spec or change proposal; no task runs
  until the spec is approved and planned.

### SDD commands

```bash
scrumforge spec new <slug> --title "<title>" [--from "<request>"]
scrumforge spec list [--status <status>] [--json]
scrumforge spec show <slug> [--json]
scrumforge spec approve <slug>
scrumforge spec plan <slug>
scrumforge spec export <slug> [--dir specs/<slug>]
scrumforge spec archive <slug>
scrumforge change new <slug> --spec <spec-slug> --title "<title>"
scrumforge change list [--status <status>] [--json]
scrumforge change approve <slug>
```

### Frontends and utility commands

```bash
scrumforge                    # TUI, project picker if unresolved
scrumforge repl               # REPL, same project context
scrumforge serve [--port 3030]    # web UI and API on loopback
scrumforge --web [--port 3030]    # alias for serve
scrumforge context [--json]   # resolved project/profile/sprint for agents
scrumforge worktree list|prune
scrumforge db backup [--out <path>]
scrumforge db check
scrumforge help [topic]
```

### Compatibility and migration

- Existing verbs (`backlog`, `tasks`, `show`, `assign`, `run`, `rework`,
  `review`, `request`) keep their names and semantics where possible.
- Existing numeric task ids remain valid globally.
- When a project has no active sprint, `tasks` defaults to the product backlog and
  `board` asks for a sprint; it does not create a sprint silently.
- `project migrate <path> --route <route>` imports a legacy `.scrumforge.db`:
  - Creates the project if it does not exist.
  - Creates a completed sprint named `Imported` and attaches all non-backlog
    legacy tasks to it, preserving their statuses.
  - Leaves legacy backlog tasks in the product backlog.
  - Copies `assignee` strings into `assignee_profile_id` when a matching profile
    exists, otherwise preserves the string in event detail and leaves unassigned.
  - Records one `imported` event per task.
- Legacy `.scrumforge.db` files are never updated after migration. They can be
  archived or removed by the user.
- `.scrumforge.json` is not read; if encountered, the CLI explains that it is
  obsolete and stops.

## Agent roles and protocols

`GUIDE.md` remains the role-playbook document and must be updated with the new
contracts when implementation begins. The spec expects these roles:

| Role | Methodology | Responsibility |
| --- | --- | --- |
| Scrum master | Scrum | Turn a PO request into backlog tasks and assignments |
| Spec planner | SDD | Draft specs, plans, requirements, and linked tasks |
| Developer | Both | Implement one task in a worktree, commit, reply |
| Code reviewer | Both | Review diff, verify acceptance criteria/spec coverage, merge or request changes |
| Product owner | Both | Approve specs, set sprint goals, accept work |

Every agent reply remains a single JSON object, as today. The prompt envelope adds
`project`, `project_route`, `sprint`, `task`, `spec`, `change`, `worktree`, and
`base_branch`. The reply contracts are extended as follows:

```json
{
  "tasks": [
    {
      "title": "short imperative summary",
      "description": "what done means",
      "acceptance_criteria": ["observable criterion"],
      "assignee": "developer",
      "spec_slug": "user-pagination",
      "requirement_keys": ["R-1"],
      "estimate": 2
    }
  ]
}
```

```json
{
  "spec": {
    "slug": "user-pagination",
    "title": "Paginate the user list",
    "summary": "why this change exists",
    "requirements": [
      { "key": "R-1", "kind": "functional", "text": "..." }
    ],
    "non_goals": ["..."],
    "acceptance_criteria": ["..."]
  },
  "plan": {
    "approach": "...",
    "affected_areas": ["src/api/users.rs"],
    "risks": ["..."],
    "test_strategy": "..."
  }
}
```

```json
{
  "verdict": "approve",
  "notes": "safe to merge",
  "requirement_coverage": [
    { "key": "R-1", "status": "verified", "evidence": "test at ..." }
  ]
}
```

Scrumforge validates references before writing: a task may not link to an
unapproved spec, and a reviewer may not approve an SDD task whose required
requirement links have no coverage evidence.

## Phasing

| Phase | Work | Unlocks |
| --- | --- | --- |
| **0** | Extract `core`; single-store schema and migrations; project record; route validation; resolver; `--project`/`--db` parsing; legacy migration | One DB, explicit project targeting |
| **1** | Profiles, actors, events, claims, leases; shared action layer over the store | Identity, audit trail, safe attribution |
| **2** | Sprints, backlog, sprint board, sprint close; TUI and web navigation per sprint | Scrum methodology and Jira-like views |
| **3** | SDD documents, requirements, specs, changes, planning, verification, export; SDD views | SDD methodology |
| **4** | Concurrent run queue, per-project limit, merge lock, worktree lifecycle commands | Many worktrees per project safely |
| **5** | Streaming agent output and SSE progress; activity feeds; dashboard polish | Live observable runs |

Phases 0 and 1 are prerequisites. Phase 2 delivers the requested sprints and
board-per-sprint behavior. Phase 3 delivers SDD. Phase 4 delivers the requested
multi-worktree concurrency.

## Consequences

### Positive

- One queryable store makes cross-project dashboards, activity feeds, and
  per-agent views straightforward.
- Routes give every project a stable name for CLI, web, agent prompts, and logs.
- Sprints and backlog become first-class instead of being encoded in one board.
- SDD artifacts and traceability share the same store as execution, so a spec's
  status can be derived from linked tasks.
- Concurrent worktrees become safe because claims, events, and merges are
  coordinated centrally.
- All frontends remain additive over a shared core; the current visual design is
  preserved.

### Negative

- A central database is a single point of failure; it needs backup and migration
  care.
- Boards are no longer committable by default; projects that want repo-visible
  specs must use `spec export`.
- Routes and path bindings can drift when repositories move; resolution errors
  must be explicit.
- More schema and more CLI verbs increase the surface area.
- Concurrent agents introduce merge and rebase complexity that did not exist with
  a single-task-at-a-time workflow.

### Neutral

- `AGENTS.md` currently claims a JSON board at `.scrumforge.json`; implementation
  must correct those stale statements to SQLite and the central store.
- The old web API was single-project and cwd-scoped; project-scoped APIs replace
  it.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Repository moved or deleted | Lazy path validation; `project set --path`; `project prune`; clear errors |
| Two agents claim one task | Conditional claim inside `BEGIN IMMEDIATE`; lease expiry |
| Two PRs race the base branch | Per-project merge lock; rebase before push |
| Route collision or rename breaks automation | Required unique routes; explicit `rename-route`; report stale bindings |
| Legacy data loss during migration | `project migrate` is explicit, transactional, and records events; backup first |
| Sprint close loses unfinished work | Require an explicit target sprint or backlog selection |
| SDD specs diverge from code | DB is canonical; `spec export` produces repo-visible copies; reviewer checks requirement coverage |
| Single SQLite file corrupted | WAL, `db check`, `db backup`, and documented restore |
| Agents act on the wrong project | `--project` required in prompts; `SCRUMFORGE_PROJECT`; `scrumforge context`; no silent cwd guessing |

## Open questions

- Should `route` be immutable forever, or should `rename-route` rewrite known
  `.scrumforge.toml` files automatically?
- Should estimates be story points (`integer`) or time (`minutes`), and how
  should sprint capacity be expressed?
- Should SDD support a "continuous" mode with a long-lived sprint, or is every
  SDD project required to use timeboxed sprints?
- Should a project be able to enable SDD on top of Scrum (hybrid), or is
  `scrum` vs `sdd` an exclusive choice for v1?
- Should the web UI support a global cross-project board, or remain
  project-scoped with only a dashboard at `/`?
- Should task identifiers gain Jira-style keys (`SCR-12`) in addition to global
  numeric ids?
- Should `serve` remain loopback-only, or gain a token for LAN access?
