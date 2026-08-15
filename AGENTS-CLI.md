# AGENTS.md — Using scrumforge as a coding agent

This guide is for **any coding agent** (Claude Code, Codex, opencode, Cursor, etc.)
that wants to drive scrumforge's non-interactive CLI to manage work on a repository.

## What scrumforge gives you

scrumforge is an AI scrum team orchestrator. As the driving agent you act as the
**product owner**: you create requests or tasks, and scrumforge dispatches them to
sub-agent roles (scrum master, developer, code reviewer) that plan, implement in
isolated git worktrees, and open/review/merge pull requests.

Everything below is non-interactive, prints a short result line, and exits — safe
to call from scripts and agent tool loops. Run it from anywhere inside the target
git repository; the repo root and its board (`.scrumforge.db`) are found
automatically.

## Commands

```bash
scrumforge tasks                          # list the board: id, status, assignee
scrumforge show <id>                      # full detail: desc, branch, PR, review notes
scrumforge backlog "<title> | <desc>"     # add a task directly (Backlog)
scrumforge request "<text>"               # scrum master plans 1-4 tasks + assigns
scrumforge assign <id> <who>              # set assignee; Backlog -> Assigned
scrumforge run <id>                       # send task to its assignee
scrumforge review <id> "<feedback>"       # human/agent review: back to InProgress
scrumforge rework <id>                    # developer addresses review feedback
```

## Lifecycle

```text
Backlog → Assigned → InProgress → InReview → Done
                         ↑             │
                         └─ ChangesRequested
```

- `run` on an assigned/in-progress task → the developer agent implements in a
  worktree (`~/dev/worktrees/<project>/<branch>`), commits, and scrumforge pushes
  and opens a PR; the task moves to InReview.
- `run` on an InReview task → the reviewer agent reads the PR diff and either
  approves (squash-merge + worktree cleanup → Done) or requests changes (task back
  to the developer with notes).
- `review` lets you (the PO/agent) send any task back to InProgress with your own
  feedback stored in its review notes.
- `rework` after changes-requested → the developer rebases, fixes, and pushes.

## Suggested agent loop

```bash
scrumforge request "add pagination to the user list endpoint"
scrumforge tasks            # see what the scrum master created
scrumforge run 1            # developer implements, PR opened
scrumforge run 1            # reviewer approves+merges or requests changes
scrumforge show 1           # check PR URL / review notes
scrumforge rework 1         # only if changes were requested
```

## Notes for agents

- Agent runs can take minutes — `run` blocks until the role finishes. Call it
  sequentially, not in parallel.
- Only one task should be `run` at a time per repo; each gets its own branch
  `scrumforge/task-<id>` based on `main`.
- `review` overrides the automated reviewer: use it when you want a human/agent
  gate before merge instead of auto-merge.
- Check `scrumforge show <id>` after every step; the board is the source of truth.
