//! `scrumforge help` — the long-form guide to everything scrumforge can do.
//!
//! `-h` / `--help` keeps the terse usage in [`crate::cli::HELP`]; this module is
//! the explanatory version, optionally narrowed to one topic.

const OVERVIEW: &str = "\
OVERVIEW
    scrumforge runs an AI scrum team over one git repository. You are the
    product owner: you describe what you want. AI agents (invoked as
    non-interactive `opencode run` turns) play the other roles.

      scrum master   splits a request into tasks and assigns each one
      developer      implements a task in an isolated git worktree,
                     commits, pushes, and opens a pull request
      reviewer       reads the PR diff, then squash-merges it or sends the
                     task back with written feedback

    Every repository keeps its own board in a SQLite file at the repo root,
    `.scrumforge.db`. scrumforge walks up from your working directory to find
    the repo root, so you can run it from any subdirectory.
";

const LIFECYCLE: &str = "\
LIFECYCLE
    backlog ─> assigned ─> in-progress ─> in-review ─> done
                              ^               │
                              └─ changes-requested

    backlog             created, nobody owns it yet
    assigned            has an assignee, work has not started
    in-progress         the developer is implementing (or reworking) it
    in-review           a PR exists and is waiting on the reviewer
    changes-requested   the reviewer asked for changes; `rework` picks it up
    done                the PR was approved and squash-merged

    Which command moves a task depends on who owns it: `run` dispatches to the
    assignee, so `run` on a developer task implements it, and `run` on an
    in-review task triggers the reviewer.
";

const CLI: &str = "\
COMMANDS
    scrumforge
        Interactive TUI board. This is the default with no arguments.

    scrumforge repl
        Line-oriented REPL with the same operations, for when a full-screen
        board is inconvenient (ssh, logs, narrow terminals).

    scrumforge backlog <title> [| <description>]
        Add a task straight to the backlog, no agent involved. Everything
        after the first `|` becomes the description.
          scrumforge backlog \"fix login bug\" | \"session expires immediately\"

    scrumforge request <text>
        Hand a plain-language request to the scrum master agent. It splits the
        work into tasks, assigns each to the developer or reviewer, and puts
        them on the board as `assigned`. Prints one line per created task.
          scrumforge request \"add pagination to the user list endpoint\"

    scrumforge tasks
        List the whole board: id, status, assignee, title.

    scrumforge show <id>
        Full detail for one task — description, assignee, branch, PR url, and
        any review feedback attached to it.

    scrumforge assign <id> <developer|reviewer|name>
        Set the assignee. A task sitting in `backlog` moves to `assigned`.

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

    scrumforge help [topic]
        This guide. Topics: overview, lifecycle, commands, tui, repl, agents,
        files, requirements.

    scrumforge --help          terse usage summary
    scrumforge --version       print the version
";

const TUI: &str = "\
TUI
    Columns are the statuses; each card is a task.

      hjkl / arrows   move between columns and tasks
      Enter           open the selected task in a modal
                      (inside it: `r` run, `w` rework, `v` review, Esc close)
      r               run the selected task (send it to its assignee)
      w               rework the selected task after changes were requested
      v               review: type feedback, task goes back in-progress
      R               ask the scrum master to plan a request
      a               add a backlog task (`title | description`)
      :               command mode — run 3, rework 3, assign 3 reviewer,
                      review 3 <feedback>, quit
      ?  or  F1       help overlay
      q               quit; Esc closes a modal, overlay, or prompt

    Agent turns take minutes. While one runs, a busy overlay is shown and
    Ctrl-C aborts it; board state is written after each step, so an aborted
    run leaves the task where it got to rather than losing it.
";

const REPL: &str = "\
REPL
    `scrumforge repl` reads one command per line, same verbs as the CLI:

      backlog \"title\" | \"description\"    add a task directly
      request <text>                      scrum master plans + assigns
      tasks                               show the board
      run <id>                            send task to its assignee
      rework <id>                         developer addresses feedback
      assign <id> <who>                   assign (backlog -> assigned)
      review <id> <feedback>              send back in-progress with notes
      help                                list these commands
      quit                                exit
";

const AGENTS: &str = "\
AGENTS
    Each agent turn is one non-interactive `opencode run` invocation. The
    agent must answer with a single JSON object; scrumforge parses that reply
    and updates the board from it. The role playbooks and the exact reply
    contracts live in GUIDE.md.

    Developer
        Works in a worktree at `~/dev/worktrees/<project>/<branch>` — always
        outside the repo tree, so stray files in the checkout cannot leak into
        the build. Commits with Conventional Commits, then scrumforge pushes
        the branch and opens the PR with `gh`.

    Reviewer
        Reads the PR diff. On approve, scrumforge squash-merges the PR and
        removes the worktree. On changes, the feedback is stored on the task
        and the task waits for `rework`.

    Scrum master
        Turns one product-owner request into a list of tasks with assignees.

    Note: the reviewer merges automatically on approve — there is no human
    confirmation gate yet — and the base branch is hard-coded to `main`.
";

const FILES: &str = "\
FILES
    <repo>/.scrumforge.db          the board for that repo (SQLite)
    ~/dev/worktrees/<proj>/<br>    developer worktrees, one per task branch
    ~/.local/bin/scrumforge        symlink created by install.sh

    AGENTS-CLI.md                  CLI guide for coding agents using scrumforge
    GUIDE.md                       role playbooks for the agents it hires
    AGENTS.md                      guidance for agents working ON this repo
";

const REQUIREMENTS: &str = "\
REQUIREMENTS
    - a git repository you can push branches to
    - `opencode` on PATH (it runs the agents)
    - `gh` authenticated (`gh auth login`) for the pull request flow

    Run from anywhere inside the repository; the root is found automatically.
";

const TOPICS: &[(&str, &str)] = &[
    ("overview", OVERVIEW),
    ("lifecycle", LIFECYCLE),
    ("commands", CLI),
    ("tui", TUI),
    ("repl", REPL),
    ("agents", AGENTS),
    ("files", FILES),
    ("requirements", REQUIREMENTS),
];

/// Aliases so `help cli`, `help board`, `help status` etc. land somewhere sane.
fn resolve(topic: &str) -> Option<&'static str> {
    let topic = topic.trim().to_ascii_lowercase();
    let canonical = match topic.as_str() {
        "cli" | "command" | "commands" => "commands",
        "status" | "statuses" | "board" | "flow" | "lifecycle" => "lifecycle",
        "agent" | "agents" | "roles" | "opencode" => "agents",
        "keys" | "tui" | "keybindings" => "tui",
        "file" | "files" | "db" | "database" => "files",
        "requirements" | "install" | "setup" => "requirements",
        other => other,
    };
    TOPICS
        .iter()
        .find(|(name, _)| *name == canonical)
        .map(|(_, body)| *body)
}

/// Print the full guide, or one topic when `args` names one.
pub fn print(args: &[String]) {
    println!(
        "scrumforge {} — AI scrum team orchestrator\n",
        env!("CARGO_PKG_VERSION")
    );

    if let Some(topic) = args.first() {
        match resolve(topic) {
            Some(body) => print!("{body}"),
            None => {
                let names: Vec<&str> = TOPICS.iter().map(|(n, _)| *n).collect();
                println!("unknown help topic: {topic}");
                println!("topics: {}", names.join(", "));
            }
        }
        return;
    }

    for (i, (_, body)) in TOPICS.iter().enumerate() {
        if i > 0 {
            println!();
        }
        print!("{body}");
    }
}
