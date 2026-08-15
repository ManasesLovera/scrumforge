use anyhow::{bail, Result};
use rustyline::DefaultEditor;

use crate::board::Board;
use crate::ops;

fn usage() {
    println!(
        "commands:\
         \n  backlog \"title\" | \"description\"   add a task directly (PO)\
         \n  request <text>              ask the scrum master to plan + assign\
         \n  tasks                       show the board\
         \n  run <id>                    send task to its assignee (dev implements, reviewer reviews)\
         \n  rework <id>                 developer addresses review feedback\
         \n  assign <id> <who>           assign a task (backlog -> assigned)\
         \n  review <id> <feedback>      send task back to in-progress with feedback\
         \n  quit"
    );
}

pub fn run(board: Board) -> Result<()> {
    let mut board = board;
    let mut rl = DefaultEditor::new()?;
    println!(
        "scrumforge repl — you are the product owner. repo: {}",
        board.repo_path.display()
    );
    usage();

    while let Ok(line) = rl.readline("po> ") {
        rl.add_history_entry(&line)?;
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let (cmd, rest) = match line.split_once(' ') {
            Some((c, r)) => (c, r.trim()),
            None => (line.as_str(), ""),
        };
        let result: Result<()> = (|| {
            match cmd {
                "quit" | "exit" => std::process::exit(0),
                "backlog" => println!("{}", ops::add_backlog_task(&mut board, rest)?),
                "request" => {
                    println!("scrum master is planning…");
                    for l in ops::request(&mut board, rest)? {
                        println!("  {l}");
                    }
                }
                "tasks" => {
                    let tasks = board.tasks();
                    for t in &tasks {
                        crate::agents::print_task(t);
                    }
                    if tasks.is_empty() {
                        println!("(board is empty)");
                    }
                }
                "run" => {
                    let id: u32 = rest.parse()?;
                    println!("working on task #{id}…");
                    println!("{}", ops::run_task(&mut board, id)?);
                }
                "rework" => {
                    let id: u32 = rest.parse()?;
                    println!("{}", ops::rework(&mut board, id)?);
                }
                "assign" => {
                    let (id, who) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("usage: assign <id> <who>"))?;
                    let id: u32 = id.parse()?;
                    println!("{}", ops::assign(&mut board, id, who)?);
                }
                "review" => {
                    let (id, feedback) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("usage: review <id> <feedback>"))?;
                    let id: u32 = id.parse()?;
                    println!("{}", ops::review(&mut board, id, feedback)?);
                }
                "help" => usage(),
                other => bail!("unknown command: {other}"),
            }
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("error: {e:#}");
        }
    }
    Ok(())
}
