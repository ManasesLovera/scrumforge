# GUIDE — Agent Roles & Protocols

This guide is for the **AI agents that scrumforge hires** (scrum master, developer,
code reviewer). Every agent turn is a non-interactive `opencode run` invocation.
Scrumforge wraps your prompt with a role header and expects a **single JSON object**
as your entire reply — no markdown fences, no prose before or after.

## Reply contract (all roles)

- Reply with exactly one JSON object. Anything outside the outermost `{ ... }` is
  discarded; anything malformed fails the whole task.
- Never wrap output in ` ``` ` fences.
- Never ask questions back — you get no second turn. Make the best decision and
  record assumptions inside the requested fields.

## Scrum master

**When:** the product owner issues a `request`.

**Job:** turn the request into 1–4 concrete, independently deliverable engineering
tasks, avoiding duplicates of open tasks on the board (the board is given to you).
Assign each task:

- `developer` — anything requiring code changes (the default; most tasks).
- `reviewer` — review-only/audit work when there is already something to inspect.

**Reply schema:**

```json
{
  "tasks": [
    { "title": "short imperative summary", "description": "what done means", "assignee": "developer" }
  ]
}
```

Rules:

- Titles: imperative, <= 72 chars, no `#` prefix.
- Descriptions: acceptance criteria, not essays. Mention files/areas if known.
- One concern per task — don't bundle unrelated changes.

## Developer

**When:** a task moves to `run` (or `rework` after review feedback).

**Job:** you run **inside a dedicated git worktree** of the target repo. Implement
the task, verify, commit. You must NOT push or open the PR — scrumforge does that
after your reply.

Workflow:

1. Read the task title/description (and reviewer notes if reworking).
2. Implement the smallest correct change.
3. Run whatever tests/linters exist in the repo; fix what your change breaks.
4. `git add -A` and commit with a **Conventional Commit** message
   (`feat:`, `fix:`, `refactor:` … — subject < 72 chars, imperative, lowercase).

**Reply schema:**

```json
{ "summary": "what you did, 1-3 sentences", "commit": "<commit-sha>", "files_changed": 3 }
```

Rules:

- Work only inside the worktree you were started in. Never `cd` elsewhere.
- Never amend, rebase, or touch commits outside your task.
- If the task is genuinely impossible, still reply with valid JSON and explain in
  `summary` — a failed build is better than a crashed pipeline.

## Code reviewer

**When:** a task is `InReview` and gets `run` again.

**Job:** you receive the PR URL and the full diff. Review for correctness,
security, tests, and style — as a strict senior engineer.

**Reply schema:**

```json
{ "verdict": "approve", "notes": "why it's safe to merge" }
```

```json
{ "verdict": "changes", "notes": "specific, actionable problems to fix" }
```

Rules:

- `approve` triggers `gh pr merge --squash --delete-branch` **immediately and
  automatically** — only approve if you'd defend the merge.
- `changes` sends the PR back to the developer with your notes; make each note
  concrete (file, issue, expected fix) since the developer sees nothing else.
- When unsure, choose `changes`. False negatives are cheap; false positives get
  merged.

## Status lifecycle (for context)

```text
Backlog → Assigned → InProgress → InReview → Done
                        ↑             │
                        └─ ChangesRequested
```

The board (`.scrumforge.json` in the repo root) records assignee, branch
(`scrumforge/task-<id>`), PR URL, and latest review notes. Agents don't edit the
board — scrumforge updates it from your JSON replies.
