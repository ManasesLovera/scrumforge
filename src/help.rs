//! `scrumforge help` — the long-form guide to everything scrumforge can do.
//!
//! `-h` / `--help` keeps the terse usage in [`crate::cli::HELP`]; this module is
//! the explanatory version, optionally narrowed to one topic.

const OVERVIEW: &str = "\
OVERVIEW
    scrumforge runs an AI scrum team over one or more git repositories. You
    are the product owner: you describe what you want. AI agents (invoked as
    non-interactive `opencode run` turns) play the other roles.

      scrum master   splits a Scrum request into backlog tasks
      spec planner   drafts a specification, plan, and requirements (SDD)
      developer      implements a task in an isolated git worktree,
                     commits, pushes, and opens a pull request
      reviewer       reads the PR diff, then squash-merges it or sends the
                     task back with written feedback

    One global SQLite store holds every registered project, sprint, task,
    spec, and event. A project is identified by a required URL-safe route,
    e.g. `acme`; the web UI addresses it as `/p/acme/...`. Work is always
    sprint-scoped, while unscheduled tasks sit in the product backlog.

    Use `--project <route>` to target a project. Without it, scrumforge uses
    `SCRUMFORGE_PROJECT`, the nearest `.scrumforge.toml`, or the registered
    repository containing your working directory.
";

const LIFECYCLE: &str = "\
LIFECYCLE
    backlog ─> todo ─> in-progress ─> in-review ─> done
                          ^               │
                          └─ changes-requested

    backlog             unscheduled product-backlog item
    todo                scheduled in a sprint, not started
    in-progress         the developer is implementing (or reworking) it
    in-review           a PR exists and is waiting on the reviewer
    changes-requested   the reviewer asked for changes; `rework` picks it up
    done                the PR was approved and squash-merged
    cancelled           abandoned, retained in history

    A task in `backlog` must be moved into a sprint before it can run.
    `run` dispatches to the assignee, so `run` on a developer task implements
    it, and `run` on an in-review task triggers the reviewer.
";

const PROJECTS: &str = "\
PROJECTS
    Register a repository exactly once. The route is required and permanent
    unless renamed; it appears in web URLs and agent prompts.

      scrumforge project add acme ~/code/acme --name \"Acme\"
      scrumforge project add docs ~/code/docs --methodology sdd
      scrumforge project list
      scrumforge project show acme
      scrumforge project set acme --default-branch develop
      scrumforge project archive acme
      scrumforge project rm acme --yes
      scrumforge project rename-route acme acme-app
      scrumforge project migrate ~/code/legacy --route legacy

    Routes are lowercase letters, digits, and hyphens, 2-48 characters, and
    must start and end alphanumeric. Reserved routes include `api`, `assets`,
    `static`, `p`, `admin`, `health`, `events`, `projects`, and `new`.

    Defaults: base branch `main`, worktree root `~/dev/worktrees`, at most 2
    parallel agent runs per project. `.scrumforge.toml` in a repo may pin
    `route = \"acme\"` (and optionally `default_profile`).
";

const SPRINTS: &str = "\
SPRINTS
    Work belongs to exactly one sprint. Only one sprint per project can be
    active at a time.

      scrumforge sprint add --name \"Sprint 1\" --goal \"Ship login\"
      scrumforge sprint list [--state planned|active|completed]
      scrumforge sprint show current
      scrumforge sprint activate 1
      scrumforge sprint close 1 --move-unfinished backlog
      scrumforge sprint board 1

    When no sprint is active, `tasks` defaults to the product backlog and the
    board asks you to plan a sprint. Closing a sprint moves unfinished tasks
    to the product backlog by default.

    Backlog:
      scrumforge backlog add \"fix login bug\" \"session expires immediately\"
      scrumforge backlog add \"fix signup\" --sprint 1
      scrumforge backlog list
      scrumforge move <id> sprint:1
      scrumforge move <id> todo
";

const CLI: &str = "\
COMMANDS
    scrumforge
        Interactive TUI. This is the default with no arguments.

    scrumforge repl
        Line-oriented REPL with the same operations, for when a full-screen
        board is inconvenient (ssh, logs, narrow terminals).

    scrumforge backlog add <title> [<description>]
        Add a task straight to the product backlog, no agent involved.

    scrumforge request <text>
        Hand a plain-language request to the planner. Scrum projects get
        backlog tasks from the scrum master; SDD projects get a draft spec.

    scrumforge tasks [--sprint current|backlog|all|<n>]
        List tasks. Defaults to the active sprint, or the backlog when no
        sprint is active.

    scrumforge show <id>
        Full detail for one task — description, acceptance criteria, sprint,
        assignee, branch, PR url, spec link, and review feedback.

    scrumforge assign <id> <developer|reviewer|name>
        Set the assignee.

    scrumforge move <id> <status|sprint:<n>>
        Apply a status transition, or schedule the task into a sprint with
        `sprint:<n>` (or `sprint:current`). Moving to `backlog` clears the
        sprint.

    scrumforge run <id>
        Dispatch the task to whoever it is assigned to.
          developer  → creates the worktree, implements, commits, pushes,
                       opens the PR, and moves the task to in-review
          reviewer   → reads the PR diff, then either approves, squash-merges
                       and cleans up the worktree (task → done), or requests
                       changes (task → changes-requested)
        Unassigned tasks default to the developer.

    scrumforge rework <id>
        Send a task with requested changes back to the developer, who
        addresses the feedback on the same branch and pushes again.

    scrumforge review <id> <feedback>
        Your own review as product owner: attaches the feedback to the task
        and puts it back in-progress. This is the human path — it does not
        invoke the reviewer agent.

    scrumforge serve [--port 3030]
        Web UI and JSON API on loopback. Opens the global project dashboard.

    scrumforge context [--json]
        Show the resolved project, profile, and active sprint.

    scrumforge activity [--limit N]
        Project event feed.

    scrumforge worktree list|prune
        Inspect or clean developer worktrees.

    scrumforge db backup|check
        Back up or verify the global SQLite store.

    scrumforge help [topic]
        This guide. Topics: overview, lifecycle, projects, sprints, commands,
        sdd, web, tui, repl, agents, files, requirements.

    scrumforge --help          terse usage summary
    scrumforge --version       print the version
";

const SDD: &str = "\
SDD
    SDD projects turn requests into specs before work. `request` creates a
    draft spec; nothing runs until it is approved and planned.

      scrumforge spec new <slug> --title \"<title>\" [--from \"<request>\"]
      scrumforge spec list [--status <status>]
      scrumforge spec show <slug>
      scrumforge spec approve <slug>
      scrumforge spec plan <slug>
      scrumforge spec export <slug>
      scrumforge spec archive <slug>

      scrumforge change new <slug> --spec <spec-slug> --title \"<title>\"
      scrumforge change list
      scrumforge change approve <slug>

    `spec plan` creates one backlog task per requirement, linked by
    `spec_slug` and `requirement_key`, then marks the spec `implementing`.
    `spec export` writes `specs/<slug>/{spec,plan,tasks}.md` into the repo.
";

const WEB: &str = "\
WEB
    `scrumforge serve` (or `--web`) starts an axum server on 127.0.0.1 and
    serves an embedded dashboard. It does not require a current project.

      /                              global project dashboard
      /p/<route>                     project overview
      /p/<route>/backlog             product backlog
      /p/<route>/sprints             sprint list
      /p/<route>/sprints/<number>    sprint board
      /p/<route>/specs               SDD specs
      /p/<route>/specs/<slug>        spec detail
      /p/<route>/activity            project event feed

    The JSON API lives under `/api/projects`; task ids are global. The board
    is sprint-scoped: drag cards between Todo, In progress, Review, and Done.
    Moving a card to the backlog clears its sprint.
";

/// TUI key bindings. Rendered as prose by `help tui` and as the in-app `?`
/// overlay ([`crate::tui`]), so the two can never drift apart. Keep the
/// descriptions short enough to fit the overlay box.
pub const KEYS: &[(&str, &str)] = &[
    ("←/h →/l", "select column"),
    ("↑/k ↓/j", "select task"),
    ("Enter", "open task modal (r run, w rework, v review)"),
    ("r", "send task to its assignee"),
    ("w", "developer reworks after changes requested"),
    ("v", "review: send task back in progress with feedback"),
    ("R", "ask the planner for a request"),
    ("a", "add a backlog task"),
    ("b", "toggle board / backlog view"),
    ("s", "cycle active sprint"),
    (":", "command mode (run, rework, assign, review, quit)"),
    ("? / F1", "this help"),
    ("q", "quit (Esc closes modals)"),
];

/// The board flow, as shown in the `?` overlay.
pub const FLOW: &[&str] = &[
    "request → backlog → sprint → todo → run → in-review",
    "  → run (reviewer) → merged ✓  or changes-requested → w → in-review …",
];

/// Caveats worth repeating wherever the keys are shown.
pub const TUI_NOTES: &[&str] = &[
    "agent turns take minutes; Ctrl-C aborts a running one",
    "board state is stored in the central SQLite database after each step",
];

/// Column width for the key names in both renderings.
pub const KEY_WIDTH: usize = 9;

fn tui() -> String {
    let mut s = String::from(
        "TUI\n    The board is sprint-scoped; `b` switches to the product backlog.\n\n",
    );
    for (key, desc) in KEYS {
        s.push_str(&format!("      {key:<width$} {desc}\n", width = KEY_WIDTH));
    }
    s.push_str("\n    Flow:\n");
    for line in FLOW {
        s.push_str(&format!("      {line}\n"));
    }
    s.push('\n');
    for note in TUI_NOTES {
        s.push_str(&format!("    - {note}\n"));
    }
    s
}

const REPL: &str = "\
REPL
    `scrumforge repl` reads one command per line, same verbs as the CLI:

      backlog \"title\" | \"description\"    add a backlog task
      request <text>                      planner creates work
      tasks [backlog|all|<n>]             show tasks
      show <id>                           full task detail
      sprint list                         list sprints
      sprint activate <n>                 activate a sprint
      sprint close <n> [backlog]          close a sprint
      spec list | show <slug>             SDD specs
      run <id>                            send task to its assignee
      rework <id>                         developer addresses feedback
      assign <id> <who>                   assign a task
      review <id> <feedback>              send back in-progress with notes
      move <id> <status>                  move a task
      help                                list these commands
      quit                                exit
";

const AGENTS: &str = "\
AGENTS
    Each agent turn is one non-interactive `opencode run` invocation. The
    agent must answer with a single JSON object; scrumforge parses that reply
    and updates the store from it. The role playbooks and exact reply
    contracts live in GUIDE.md.

    Developer
        Works in a worktree at `<worktree_root>/<route>/<branch>` — by
        default `~/dev/worktrees/<route>/scrumforge/task-<id>`, always
        outside the repo tree. Commits with Conventional Commits, then
        scrumforge pushes the branch and opens the PR with `gh`.

    Reviewer
        Reads the PR diff. On approve, scrumforge squash-merges the PR and
        removes the worktree. On changes, the feedback is stored on the task
        and the task waits for `rework`.

    Scrum master
        Turns one Scrum product-owner request into backlog tasks.

    Spec planner
        Turns one SDD product-owner request into a draft spec, plan, and
        testable requirements.

    Note: the reviewer merges automatically on approve — there is no human
    confirmation gate yet. The base branch comes from the project settings.
";

const FILES: &str = "\
FILES
    ~/.local/share/scrumforge/scrumforge.db    global store (SQLite, WAL)
    $XDG_DATA_HOME/scrumforge/scrumforge.db    same, when XDG_DATA_HOME is set
    ~/.config/scrumforge/config.toml           optional database.path override
    <repo>/.scrumforge.toml                    optional project route binding
    ~/dev/worktrees/<route>/<branch>           developer worktrees
    specs/<slug>/                              `spec export` output

    AGENTS-CLI.md                  CLI guide for coding agents using scrumforge
    GUIDE.md                       role playbooks for the agents it hires
    AGENTS.md                      guidance for agents working ON this repo

    Overrides, highest priority first: --db, SCRUMFORGE_DB, config.toml,
    then the XDG default.
";

const REQUIREMENTS: &str = "\
REQUIREMENTS
    - a git repository you can push branches to
    - `opencode` on PATH (it runs the agents)
    - `gh` authenticated (`gh auth login`) for the pull request flow

    Run `scrumforge project add <route> <path>` once per repository. After
    that, `--project <route>` works from anywhere.
";

/// A topic body: either a fixed block of prose, or one rendered from the
/// structured data the TUI also draws from.
type Body = fn() -> String;

const TOPICS: &[(&str, Body)] = &[
    ("overview", || OVERVIEW.to_string()),
    ("lifecycle", || LIFECYCLE.to_string()),
    ("projects", || PROJECTS.to_string()),
    ("sprints", || SPRINTS.to_string()),
    ("commands", || CLI.to_string()),
    ("sdd", || SDD.to_string()),
    ("web", || WEB.to_string()),
    ("tui", tui),
    ("repl", || REPL.to_string()),
    ("agents", || AGENTS.to_string()),
    ("files", || FILES.to_string()),
    ("requirements", || REQUIREMENTS.to_string()),
];

/// Aliases so `help cli`, `help board`, `help status` etc. land somewhere sane.
fn resolve(topic: &str) -> Option<Body> {
    let topic = topic.trim().to_ascii_lowercase();
    let canonical = match topic.as_str() {
        "cli" | "command" | "commands" => "commands",
        "status" | "statuses" | "board" | "flow" | "lifecycle" => "lifecycle",
        "agent" | "agents" | "roles" | "opencode" => "agents",
        "keys" | "tui" | "keybindings" => "tui",
        "file" | "files" | "db" | "database" => "files",
        "requirements" | "install" | "setup" => "requirements",
        "project" | "route" | "routes" => "projects",
        "sprint" | "backlog" => "sprints",
        "spec" | "specs" | "change" | "changes" => "sdd",
        "server" | "api" | "http" => "web",
        other => other,
    };
    TOPICS
        .iter()
        .find(|(name, _)| *name == canonical)
        .map(|(_, body)| *body)
}

/// Every topic, rendered end to end.
fn all() -> String {
    TOPICS
        .iter()
        .map(|(_, body)| body())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Print the full guide, or one topic when `args` names one.
pub fn print(args: &[String]) {
    println!(
        "scrumforge {} — AI scrum team orchestrator\n",
        env!("CARGO_PKG_VERSION")
    );

    if let Some(topic) = args.first() {
        match resolve(topic) {
            Some(body) => print!("{}", body()),
            None => {
                let names: Vec<&str> = TOPICS.iter().map(|(n, _)| *n).collect();
                println!("unknown help topic: {topic}");
                println!("topics: {}", names.join(", "));
            }
        }
        return;
    }

    print!("{}", all());
}
