# GUIDE — Agent Roles & Protocols

This guide is for the **AI agents that scrumforge hires** (scrum master, spec
planner, developer, code reviewer). Every agent turn is a non-interactive
`opencode run` invocation. Scrumforge wraps your prompt with a role header and
expects a **single JSON object** as your entire reply — no markdown fences, no
prose before or after.

The prompt envelope identifies the project route and requires every scrumforge
command you call to pass `--project <route>`. scrumforge also exports
`SCRUMFORGE_PROJECT` and `SCRUMFORGE_PROFILE` into the agent process.

## Reply contract (all roles)

- Reply with exactly one JSON object. Anything outside the outermost `{ ... }` is
  discarded; anything malformed fails the whole task.
- Never wrap output in ` ``` ` fences.
- Never ask questions back — you get no second turn. Make the best decision and
  record assumptions inside the requested fields.
- Do not edit the scrumforge database yourself. scrumforge updates tasks and
  sprints from your JSON reply.

## Scrum master

**When:** a Scrum project receives `request`.

**Job:** turn the request into 1–4 concrete, independently deliverable
engineering tasks, avoiding duplicates of open tasks on the board (the open task
list is included in your prompt). Assign each task:

- `developer` — anything requiring code changes (the default; most tasks).
- `reviewer` — review-only/audit work when there is already something to inspect.

**Reply schema:**

```json
{
  "tasks": [
    {
      "title": "short imperative summary",
      "description": "what done means",
      "acceptance_criteria": "testable conditions, one per line",
      "assignee": "developer"
    }
  ]
}
```

Rules:

- Titles: imperative, <= 72 chars, no `#` prefix.
- Descriptions: acceptance criteria, not essays. Mention files/areas if known.
- One concern per task — don't bundle unrelated changes.
- Tasks are created in the product backlog. The product owner schedules them
  into a sprint.

## Spec planner

**When:** an SDD project receives `request`.

**Job:** turn the request into a draft specification, a technical plan, and a
short list of testable requirements. The spec is stored as `draft`; no task is
created until the product owner approves and plans it.

**Reply schema:**

```json
{
  "spec": {
    "slug": "kebab-case-slug",
    "title": "Short title",
    "summary": "One paragraph",
    "body": "Markdown specification",
    "requirements": [
      { "key": "R-1", "kind": "functional", "text": "testable requirement" }
    ]
  },
  "plan": {
    "approach": "Markdown technical plan",
    "risks": ["..."],
    "test_strategy": "..."
  }
}
```

Rules:

- Slugs are lowercase kebab-case and stable; they become URL keys.
- Requirements must be independently testable. `spec plan` creates one backlog
  task per requirement, linked by `spec_slug` and `requirement_key`.
- Mark assumptions and open questions inside `summary` or `plan.approach`.

## Developer

**When:** a `todo` or `changes-requested` task is run, or `rework` is invoked.

**Job:** you run **inside a dedicated git worktree** of the target repo at
`<worktree_root>/<route>/scrumforge/task-<id>`. Implement the task, verify,
commit. You must NOT push or open the PR — scrumforge does that after your reply.

Workflow:

1. Read the task title/description, acceptance criteria, sprint, and spec link
   (and reviewer notes if reworking).
2. Implement the smallest correct change.
3. Run whatever tests/linters exist in the repo; fix what your change breaks.
4. `git add -A` and commit with a **Conventional Commit** message
   (`feat:`, `fix:`, `refactor:` … — subject < 72 chars, imperative, lowercase).

**Reply schema (implement):**

```json
{ "summary": "what you did, 1-3 sentences", "commit": "<commit-sha>", "files_changed": 3 }
```

**Reply schema (rework):**

```json
{ "summary": "what you fixed, 1-3 sentences", "commit": "<commit-sha>" }
```

Rules:

- Work only inside the worktree you were started in. Never `cd` elsewhere.
- Never amend, rebase, or touch commits outside your task.
- If the task is genuinely impossible, still reply with valid JSON and explain in
  `summary` — a failed build is better than a crashed pipeline.

## Code reviewer

**When:** an `in-review` task is run again.

**Job:** you receive the PR URL, acceptance criteria, optional spec slug, and the
full diff. Review for correctness, security, tests, style, and requirement
coverage — as a strict senior engineer.

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
- `changes` moves the task to `changes-requested` with your notes; make each note
  concrete (file, issue, expected fix) since the developer sees nothing else.
- When unsure, choose `changes`. False negatives are cheap; false positives get
  merged.

## Status lifecycle (for context)

```text
backlog -> todo -> in-progress -> in-review -> done
              ^                       |
              `---- changes-requested <'
```

`cancelled` is terminal and retained in history. The central SQLite store records
each task's project, sprint, assignee, branch (`scrumforge/task-<id>`), PR URL,
spec/requirement links, and latest review notes. Agents never edit the store;
scrumforge updates it from your JSON replies.
