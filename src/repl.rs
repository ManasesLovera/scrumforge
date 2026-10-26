use anyhow::{Result, bail};
use rustyline::DefaultEditor;

use crate::agents;
use crate::board::{Project, Store};
use crate::ops;

fn usage() {
    println!(
        "commands:\
         \n  backlog \"title\" | \"description\"   add a product-backlog task\
         \n  request <text>              ask the scrum master/spec planner\
         \n  tasks [backlog|all|<n>]     show tasks (default: active sprint)\
         \n  show <id>                   full task detail\
         \n  sprint list                 list sprints\
         \n  sprint activate <n>         activate a sprint\
         \n  sprint close <n> [backlog]  close a sprint\
         \n  spec list | show <slug>     SDD specs\
         \n  run <id>                    send task to its assignee\
         \n  rework <id>                 developer addresses review feedback\
         \n  assign <id> <who>           assign a task\
         \n  review <id> <feedback>      send task back in-progress\
         \n  move <id> <status>          move a task\
         \n  help [topic]                long-form guide\
         \n  quit"
    );
}

pub fn run(store: Store, project: Project) -> Result<()> {
    let mut rl = DefaultEditor::new()?;
    let actor = "product-owner";
    println!(
        "scrumforge repl — you are the product owner. project: {} ({})",
        project.route,
        project.repo_path.display()
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
                "backlog" => println!(
                    "{}",
                    ops::add_backlog_task(&store, &project, rest, None, actor)?
                ),
                "request" => {
                    println!("planning…");
                    for l in ops::request(&store, &project, rest, actor)? {
                        println!("  {l}");
                    }
                }
                "tasks" => {
                    let tasks = match rest {
                        "backlog" => store.backlog_tasks(project.id)?,
                        "all" | "" => store.tasks(project.id)?,
                        other => match other.parse::<u32>() {
                            Ok(number) => {
                                let sprint = store
                                    .sprint_by_number(project.id, number)?
                                    .ok_or_else(|| anyhow::anyhow!("sprint {number} not found"))?;
                                store.sprint_tasks(project.id, sprint.id)?
                            }
                            Err(_) => store.tasks(project.id)?,
                        },
                    };
                    for t in &tasks {
                        agents::print_task(t);
                    }
                    if tasks.is_empty() {
                        println!("(no tasks)");
                    }
                }
                "show" => {
                    let id: u32 = rest.parse()?;
                    let t = store
                        .task(project.id, id)?
                        .ok_or_else(|| anyhow::anyhow!("task #{id} not found"))?;
                    agents::print_task(&t);
                    if !t.description.is_empty() {
                        println!("  {}", t.description);
                    }
                }
                "sprint" => {
                    let (sub, arg) = rest.split_once(' ').unwrap_or((rest, ""));
                    match sub {
                        "list" => {
                            for s in store.sprints(project.id)? {
                                println!("{} [{}] {} — {}", s.number, s.status, s.name, s.goal);
                            }
                        }
                        "activate" => {
                            let number: u32 = arg.parse()?;
                            let s = store.activate_sprint(project.id, number)?;
                            println!("activated sprint {} — {}", s.number, s.name);
                        }
                        "close" => {
                            let mut parts = arg.split_whitespace();
                            let number: u32 = parts
                                .next()
                                .ok_or_else(|| {
                                    anyhow::anyhow!("usage: sprint close <n> [backlog]")
                                })?
                                .parse()?;
                            let target = parts.next();
                            let s = store.close_sprint(project.id, number, target)?;
                            println!("closed sprint {} — {}", s.number, s.name);
                        }
                        _ => bail!("usage: sprint list|activate <n>|close <n>"),
                    }
                }
                "spec" => {
                    let (sub, arg) = rest.split_once(' ').unwrap_or((rest, ""));
                    match sub {
                        "list" => {
                            for s in store.specs(project.id)? {
                                println!("{:<28} {:<12} {}", s.slug, s.status, s.title);
                            }
                        }
                        "show" => {
                            let s = store
                                .spec(project.id, arg)?
                                .ok_or_else(|| anyhow::anyhow!("spec '{arg}' not found"))?;
                            println!("# {} ({})", s.title, s.status);
                            println!("{}", s.body_md);
                        }
                        _ => bail!("usage: spec list|show <slug>"),
                    }
                }
                "run" => {
                    let id: u32 = rest.parse()?;
                    println!("working on task #{id}…");
                    println!("{}", ops::run_task(&store, &project, id, actor)?);
                }
                "rework" => {
                    let id: u32 = rest.parse()?;
                    println!("{}", ops::rework(&store, &project, id, actor)?);
                }
                "assign" => {
                    let (id, who) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("usage: assign <id> <who>"))?;
                    let id: u32 = id.parse()?;
                    println!("{}", ops::assign(&store, &project, id, who, actor)?);
                }
                "review" => {
                    let (id, feedback) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("usage: review <id> <feedback>"))?;
                    let id: u32 = id.parse()?;
                    println!("{}", ops::review(&store, &project, id, feedback, actor)?);
                }
                "move" => {
                    let (id, status) = rest
                        .split_once(' ')
                        .ok_or_else(|| anyhow::anyhow!("usage: move <id> <status>"))?;
                    let id: u32 = id.parse()?;
                    let status = crate::board::Status::from_db(status)?;
                    println!("{}", ops::move_task(&store, &project, id, &status, actor)?);
                }
                "help" => {
                    if rest.is_empty() {
                        usage();
                    } else {
                        crate::help::print(&[rest.to_string()]);
                    }
                }
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
