# AGENTS-CLI.md — Using scrumforge as a coding agent

This guide is for **any coding agent** (Claude Code, Codex, opencode, Cursor,
etc.) that wants to drive scrumforge's non-interactive CLI to manage work on one
or more repositories.

## What scrumforge gives you

scrumforge is an AI scrum team orchestrator backed by one global SQLite store.
As the driving agent you act as the **product owner**: you create requests or
tasks, and scrumforge dispatches them to sub-agent roles (scrum master or spec
planner, developer, code reviewer) that plan, implement in isolated git
worktrees, and open/review/merge pull requests.

Everything below is non-interactive, prints a short result line, and exits — safe
to call from scripts and agent tool loops.

Projects are identified by a required **route**, not by the current directory.
Pass `--project <route>` explicitly, or rely on `SCRUMFORGE_PROJECT`, the nearest
`.scrumforge.toml` (`route = "..."`), or the registered repository containing
the current directory.

## First run

```bash
scrumforge project add acme ~/code/acme --name "Acme" --methodology scrum
scrumforge project add docs ~/code/docs --name "Docs" --methodology sdd
scrumforge project list --json
```

A project also gets a worktree root (default `~/dev/worktrees`), a base branch
(default `main`), and a maximum parallel-run setting.

## Sprint flow

Work always belongs to a sprint. Unscheduled tasks live in the product backlog.

```bash
scrumforge --project acme sprint add --name "Sprint 1" --goal "Ship login"
scrumforge --project acme sprint activate 1
scrumforge --project acme sprint board 1

scrumforge --project acme backlog add "fix login bug" "session expires immediately"
scrumforge --project acme backlog add "fix signup" --sprint 1
scrumforge --project acme backlog list
scrumforge --project acme tasks --sprint current
scrumforge --project acme tasks --sprint backlog
scrumforge --project acme move 7 sprint:1
scrumforge --project acme move 7 todo
scrumforge --project acme sprint close 1 --move-unfinished backlog
```

When no sprint is active, `tasks` defaults to the product backlog. A task in the
backlog must be moved into a sprint before it can run.

## Work commands

```bash
scrumforge --project acme tasks                # list the active sprint
scrumforge --project acme show <id>            # full task detail
scrumforge --project acme assign <id> <who>    # set assignee
scrumforge --project acme run <id>             # dispatch to assignee
scrumforge --project acme rework <id>          # developer addresses feedback
scrumforge --project acme review <id> "notes"  # PO sends it back in-progress
scrumforge --project acme request "add pagination to the user list endpoint"
```

## SDD flow

```bash
scrumforge --project docs request "add versioned API docs"
scrumforge --project docs spec list
scrumforge --project docs spec show <slug>
scrumforge --project docs spec approve <slug>
scrumforge --project docs spec plan <slug>       # one backlog task per requirement
scrumforge --project docs spec export <slug>
scrumforge --project docs change new <slug> --spec <spec-slug> --title "..."
```

`request` on an SDD project creates a draft spec, not tasks. Nothing runs until
the spec is approved and planned. Planned tasks start in the product backlog and
must be scheduled into a sprint.

## Lifecycle

```text
backlog -> todo -> in-progress -> in-review -> done
              ^                       |
              `---- changes-requested <'
```

- `run` on a `todo` or `changes-requested` task → the developer implements in a
  worktree (`<worktree_root>/<route>/scrumforge/task-<id>`), commits, and
  scrumforge pushes and opens a PR; the task moves to `in-review`.
- `run` on an `in-review` task → the reviewer reads the PR diff and either
  approves (squash-merge + worktree cleanup → `done`) or requests changes (task
  moves to `changes-requested` with notes).
- `review` lets you (the PO/agent) send a task back to `in-progress` with your
  own feedback, without invoking the reviewer agent.
- `rework` after changes-requested → the developer rebases, fixes, and pushes.

## Suggested agent loop

```bash
scrumforge --project acme request "add pagination to the user list endpoint"
scrumforge --project acme tasks
scrumforge --project acme run 1
scrumforge --project acme run 1
scrumforge --project acme show 1
scrumforge --project acme rework 1     # only if changes were requested
```

## Notes for agents

- Agent runs can take minutes — `run` blocks until the role finishes.
- Task ids are globally unique, so use the same id with the project route that
  owns it.
- Branches are `scrumforge/task-<id>` and are based on the project's configured
  base branch.
- Check `scrumforge --project <route> show <id>` after every step; the store is
  the source of truth.
- Use `scrumforge context --json` to discover the resolved project, profile, and
  active sprint.
- `--json` works on the list/show commands that support it for machine parsing.
