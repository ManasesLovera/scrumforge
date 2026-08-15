use anyhow::{bail, Result};

use crate::board::Board;
use crate::ops;

pub const HELP: &str = "\
scrumforge 0.1.0 — AI scrum team orchestrator

USAGE:
    scrumforge [COMMAND] [ARGS]

    Run from anywhere inside a git repository; the repo root is found
    automatically and its .scrumforge.db board is used.

COMMANDS:
    (no command)                interactive TUI (humans)
    repl                        interactive line REPL (humans)

    Non-interactive CLI (for scripts and AI agents):
      backlog <title> [| <description>]   add a task directly
      request <text>                      ask the scrum master to plan + assign
      tasks                               list all tasks (id, status, assignee)
      show <id>                           full details of one task
      run <id>                            send task to its assignee
                                          (developer implements + opens PR;
                                          reviewer approves+merges or requests
                                          changes)
      rework <id>                         developer addresses review feedback
      help                                show this help

OPTIONS:
    -h, --help                 show this help
    -V, --version              show version

EXAMPLES:
    scrumforge request \"add pagination to the user list endpoint\"
    scrumforge run 1            # developer works, PR opened
    scrumforge run 1            # reviewer merges or requests changes
    scrumforge rework 1         # after changes requested
    scrumforge tasks

LIFECYCLE:
    backlog -> assigned -> in-progress -> in-review -> done
                                   ^          |
                                   `- changes-requested
";

pub fn run(mut board: Board, cmd: &str, args: &[String]) -> Result<()> {
    match cmd {
        "backlog" => {
            let rest = args.join(" ");
            println!("{}", ops::add_backlog_task(&mut board, &rest)?);
        }
        "request" => {
            let text = args.join(" ");
            if text.is_empty() {
                bail!("usage: scrumforge request <text>");
            }
            let mut b = board;
            eprintln!("scrum master is planning…");
            for line in ops::request(&mut b, &text)? {
                println!("{line}");
            }
        }
        "tasks" => {
            let tasks = board.tasks();
            if tasks.is_empty() {
                println!("(board is empty)");
            }
            for t in &tasks {
                crate::agents::print_task(t);
            }
        }
        "show" => {
            let id = parse_id(args)?;
            match board.get(id) {
                Some(t) => {
                    println!("#{} [{}] {}", t.id, t.status, t.title);
                    if !t.description.is_empty() {
                        println!("description: {}", t.description);
                    }
                    println!(
                        "assignee: {}",
                        t.assignee.as_deref().unwrap_or("unassigned")
                    );
                    if let Some(b) = &t.branch {
                        println!("branch: {b}");
                    }
                    if let Some(u) = &t.pr_url {
                        println!("PR: {u}");
                    }
                    if let Some(n) = t.review_notes.as_deref().filter(|n| !n.is_empty()) {
                        println!("review: {n}");
                    }
                }
                None => bail!("task #{id} not found"),
            }
        }
        "run" => {
            let id = parse_id(args)?;
            let mut b = board;
            eprintln!("working on task #{id}…");
            println!("{}", ops::run_task(&mut b, id)?);
        }
        "rework" => {
            let id = parse_id(args)?;
            let mut b = board;
            eprintln!("developer reworking task #{id}…");
            println!("{}", ops::rework(&mut b, id)?);
        }
        other => bail!("unknown command: {other} (see scrumforge --help)"),
    }
    Ok(())
}

fn parse_id(args: &[String]) -> Result<u32> {
    args.first()
        .map(|s| s.parse::<u32>())
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("missing task id (see scrumforge --help)"))
}

