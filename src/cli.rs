use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::agents;
use crate::board::{Methodology, Project, Spec, SprintStatus, Status, Store, User};
use crate::ops;
use crate::resolve::{self, validate_route};

pub const HELP: &str = "\
scrumforge 0.2.0 — AI scrum team orchestrator

USAGE:
    scrumforge [GLOBAL OPTIONS] [COMMAND] [ARGS]

    Every project-scoped command accepts --project <route>. Without it,
    scrumforge uses SCRUMFORGE_PROJECT, the nearest .scrumforge.toml, or the
    current directory's registered project.

GLOBAL OPTIONS:
    --project <route>          target a project explicitly
    --profile <id>             actor identity (default: product-owner)
    --db <path>                override the central database path
    --json                     machine-readable output
    --all                      aggregate across all projects where supported
    -h, --help                 show this help
    -V, --version              show version

PROJECTS:
    project add <route> <path> [--name N] [--methodology scrum|sdd]
                  [--default-branch main] [--worktree-root PATH]
                  [--max-parallel-runs N] [--import]
    project list | show <route> | current
    project set <route> [--name N] [--path PATH] [--methodology scrum|sdd]
    project archive <route> | rm <route> --yes
    project rename-route <old> <new>
    project migrate <path> --route <route>

USERS:
    user add <handle> [--name N] [--kind human|agent] [--scope route]...
    user list [--scope route]
    user show <handle>
    user set <handle> [--name N] [--kind human|agent] [--notes TEXT]
    user scope <handle> <route>... | --clear
    user unscope <handle> <route>...
    user rm <handle> --yes

SPRINTS:
    sprint add --name N [--goal G] [--start DATE] [--end DATE] [--capacity N]
    sprint list [--state planned|active|completed]
    sprint show <number|current|next>
    sprint activate <number>
    sprint close <number> [--move-unfinished backlog|<number>]
    sprint board <number|current> [--json]

WORK:
    backlog add \"title\" | \"description\"     add a task to the product backlog
    backlog list
    tasks [--sprint current|backlog|all|<number>] [--all]
    show <id>
    assign <id> <profile|role>
    move <id> <status>
    run <id>
    rework <id>
    review <id> \"<feedback>\"
    request \"<text>\"

SDD:
    spec new <slug> --title <title> [--from <text>]
    spec list | show <slug> | approve <slug> | plan <slug>
    spec export <slug> | archive <slug>
    change new <slug> --spec <spec-slug> --title <title> [--body <text>]
    change list | approve <slug>

OTHER:
    (no command)               interactive TUI
    repl                       line-oriented REPL
    serve [--port 3030]        web UI and API
    --web [--port 3030]        alias for serve
    context [--json]           resolved project/profile/sprint
    activity [--limit N]       project event feed
    worktree list|prune
    db backup [--out <path>] | db check
    help [topic]

LIFECYCLE:
    backlog -> todo -> in-progress -> in-review -> done
                                   ^          |
                                   `- changes-requested
";

#[derive(Debug, Clone, Default)]
pub struct Globals {
    pub project: Option<String>,
    pub profile: Option<String>,
    pub db: Option<PathBuf>,
    pub json: bool,
    pub all: bool,
}

fn value_flag(args: &[String], i: &mut usize, name: &str) -> Result<Option<String>> {
    let arg = &args[*i];
    if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
        *i += 1;
        return Ok(Some(value.to_string()));
    }
    if arg == name {
        let value = args
            .get(*i + 1)
            .with_context(|| format!("missing value for {name}"))?
            .clone();
        *i += 2;
        return Ok(Some(value));
    }
    Ok(None)
}

pub fn parse_globals(args: Vec<String>) -> Result<(Globals, Vec<String>)> {
    let mut globals = Globals::default();
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].clone();
        if let Some(value) = value_flag(&args, &mut i, "--project")? {
            globals.project = Some(value);
            continue;
        }
        if let Some(value) = value_flag(&args, &mut i, "--profile")? {
            globals.profile = Some(value);
            continue;
        }
        if let Some(value) = value_flag(&args, &mut i, "--db")? {
            globals.db = Some(resolve::expand_path(&value));
            continue;
        }
        if arg == "--json" {
            globals.json = true;
            i += 1;
            continue;
        }
        if arg == "--all" {
            globals.all = true;
            i += 1;
            continue;
        }
        rest.push(arg);
        i += 1;
    }
    Ok((globals, rest))
}

fn project(store: &Store, globals: &Globals) -> Result<Project> {
    resolve::resolve_project(store, globals.project.as_deref(), true)
}

fn actor(globals: &Globals) -> String {
    resolve::resolve_profile(globals.profile.as_deref())
}

pub fn run(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let Some(cmd) = args.first().map(String::as_str) else {
        bail!("missing command");
    };
    let rest = &args[1..];
    match cmd {
        "project" => project_cmd(store, globals, rest),
        "user" => user_cmd(store, globals, rest),
        "sprint" => sprint_cmd(store, globals, rest),
        "backlog" => backlog_cmd(store, globals, rest),
        "tasks" => tasks_cmd(store, globals, rest),
        "show" => show_cmd(store, globals, rest),
        "assign" => {
            let project = project(store, globals)?;
            let id = parse_id(rest.first())?;
            let who = rest.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            println!(
                "{}",
                ops::assign(store, &project, id, &who, &actor(globals))?
            );
            Ok(())
        }
        "move" => {
            let project = project(store, globals)?;
            let id = parse_id(rest.first())?;
            let to = rest
                .get(1)
                .context("usage: move <id> <status|sprint:<n>>")?;
            if let Some(sprint) = to.strip_prefix("sprint:") {
                let number = if sprint == "current" {
                    store
                        .active_sprint(project.id)?
                        .context("no active sprint")?
                        .number
                } else {
                    sprint.parse().context("invalid sprint number")?
                };
                println!(
                    "{}",
                    ops::add_to_sprint(store, &project, id, number, &actor(globals))?
                );
            } else {
                let target = Status::from_db(to)?;
                println!(
                    "{}",
                    ops::move_task(store, &project, id, &target, &actor(globals))?
                );
            }
            Ok(())
        }
        "run" => {
            let project = project(store, globals)?;
            let id = parse_id(rest.first())?;
            eprintln!("working on task #{id}…");
            println!("{}", ops::run_task(store, &project, id, &actor(globals))?);
            Ok(())
        }
        "rework" => {
            let project = project(store, globals)?;
            let id = parse_id(rest.first())?;
            eprintln!("developer reworking task #{id}…");
            println!("{}", ops::rework(store, &project, id, &actor(globals))?);
            Ok(())
        }
        "review" => {
            let project = project(store, globals)?;
            let id = parse_id(rest.first())?;
            let feedback = rest.get(1..).map(|s| s.join(" ")).unwrap_or_default();
            println!(
                "{}",
                ops::review(store, &project, id, &feedback, &actor(globals))?
            );
            Ok(())
        }
        "request" => {
            let project = project(store, globals)?;
            let text = rest.join(" ");
            eprintln!("planning…");
            for line in ops::request(store, &project, &text, &actor(globals))? {
                println!("{line}");
            }
            Ok(())
        }
        "spec" => spec_cmd(store, globals, rest),
        "change" => change_cmd(store, globals, rest),
        "context" => context_cmd(store, globals, rest),
        "activity" => activity_cmd(store, globals, rest),
        "worktree" => worktree_cmd(store, globals, rest),
        "db" => db_cmd(store, globals, rest),
        "help" => {
            crate::help::print(rest);
            Ok(())
        }
        other => bail!("unknown command: {other} (see scrumforge --help)"),
    }
}

fn parse_id(value: Option<&String>) -> Result<u32> {
    value
        .context("missing task id")?
        .parse::<u32>()
        .context("task id must be a number")
}

fn project_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!(
            "usage: scrumforge project <add|list|show|set|archive|rm|current|rename-route|migrate>"
        );
    };
    let rest = &args[1..];
    match sub {
        "add" => {
            let route = rest.first().context("usage: project add <route> <path>")?;
            let path = rest.get(1).context("usage: project add <route> <path>")?;
            validate_route(route)?;
            let repo_path = resolve::validate_project_path(&resolve::expand_path(path))?;
            let mut name = route.clone();
            let mut methodology = Methodology::Scrum;
            let mut default_branch = "main".to_string();
            let mut worktree_root = resolve::home_dir().join("dev").join("worktrees");
            let mut max_parallel_runs = 2u32;
            let mut import = false;
            let mut i = 2;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--name")? {
                    name = value;
                } else if let Some(value) = value_flag(rest, &mut i, "--methodology")? {
                    methodology = resolve::parse_methodology(&value)?;
                } else if let Some(value) = value_flag(rest, &mut i, "--default-branch")? {
                    default_branch = value;
                } else if let Some(value) = value_flag(rest, &mut i, "--worktree-root")? {
                    worktree_root = resolve::expand_path(&value);
                } else if let Some(value) = value_flag(rest, &mut i, "--max-parallel-runs")? {
                    max_parallel_runs = value.parse().context("invalid max parallel runs")?;
                } else if rest[i] == "--import" {
                    import = true;
                    i += 1;
                } else {
                    bail!("unknown project add option: {}", rest[i]);
                }
            }
            if store.project_by_route(route)?.is_some() {
                bail!("project route '{route}' already exists");
            }
            let project = store.add_project(
                route,
                &name,
                &repo_path,
                &default_branch,
                &worktree_root,
                &methodology,
                max_parallel_runs,
            )?;
            println!(
                "added project {} ({}) at {} [{}]",
                project.route,
                project.name,
                project.repo_path.display(),
                project.methodology
            );
            if import {
                let count = migrate_legacy(store, &project, &repo_path)?;
                println!("imported {count} legacy task(s)");
            }
            Ok(())
        }
        "list" => {
            let projects = store.projects()?;
            if globals.json {
                let values = projects.iter().map(project_json).collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&values)?);
                return Ok(());
            }
            if projects.is_empty() {
                println!("(no projects registered)");
            }
            for p in &projects {
                let active = store
                    .active_sprint(p.id)?
                    .map(|s| format!("sprint {}", s.number))
                    .unwrap_or_else(|| "no active sprint".into());
                let (backlog, done, specs, sprints) = store.counts(p.id)?;
                println!(
                    "{:<16} {:<20} {:<8} {:<14} backlog:{backlog} done:{done} sprints:{sprints} specs:{specs}",
                    p.route, p.name, p.methodology, active
                );
            }
            Ok(())
        }
        "show" => {
            let route = rest.first().context("usage: project show <route>")?;
            let project = store
                .project_by_route(route)?
                .with_context(|| format!("unknown project route '{route}'"))?;
            if globals.json {
                println!("{}", serde_json::to_string_pretty(&project_json(&project))?);
            } else {
                let (backlog, done, specs, sprints) = store.counts(project.id)?;
                println!("route:          {}", project.route);
                println!("name:           {}", project.name);
                println!("path:           {}", project.repo_path.display());
                println!("default branch: {}", project.default_branch);
                println!("worktree root:  {}", project.worktree_root.display());
                println!("methodology:    {}", project.methodology);
                println!("max parallel:   {}", project.max_parallel_runs);
                println!("backlog:        {backlog}");
                println!("done:           {done}");
                println!("sprints:        {sprints}");
                println!("specs:          {specs}");
                if let Some(sprint) = store.active_sprint(project.id)? {
                    println!("active sprint:  {} — {}", sprint.number, sprint.name);
                }
            }
            Ok(())
        }
        "set" => {
            let route = rest.first().context("usage: project set <route>")?;
            let mut project = store
                .project_by_route(route)?
                .with_context(|| format!("unknown project route '{route}'"))?;
            let mut i = 1;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--name")? {
                    project.name = value;
                } else if let Some(value) = value_flag(rest, &mut i, "--path")? {
                    project.repo_path =
                        resolve::validate_project_path(&resolve::expand_path(&value))?;
                } else if let Some(value) = value_flag(rest, &mut i, "--methodology")? {
                    project.methodology = resolve::parse_methodology(&value)?;
                } else if let Some(value) = value_flag(rest, &mut i, "--default-branch")? {
                    project.default_branch = value;
                } else if let Some(value) = value_flag(rest, &mut i, "--worktree-root")? {
                    project.worktree_root = resolve::expand_path(&value);
                } else if let Some(value) = value_flag(rest, &mut i, "--max-parallel-runs")? {
                    project.max_parallel_runs =
                        value.parse().context("invalid max parallel runs")?;
                } else {
                    bail!("unknown project set option: {}", rest[i]);
                }
            }
            store.save_project(&project)?;
            println!("updated project {}", project.route);
            Ok(())
        }
        "archive" => {
            let route = rest.first().context("usage: project archive <route>")?;
            let mut project = store
                .project_by_route(route)?
                .with_context(|| format!("unknown project route '{route}'"))?;
            project.archived_at = Some("now".into());
            store.save_project(&project)?;
            println!("archived project {}", project.route);
            Ok(())
        }
        "rm" => {
            let route = rest.first().context("usage: project rm <route> --yes")?;
            if !rest.iter().any(|a| a == "--yes") {
                bail!("refusing to delete project without --yes");
            }
            let project = store
                .project_by_route(route)?
                .with_context(|| format!("unknown project route '{route}'"))?;
            if store.active_sprint(project.id)?.is_some() {
                bail!(
                    "project '{}' has an active sprint; close it first",
                    project.route
                );
            }
            store.remove_project(project.id)?;
            println!("deleted project {}", project.route);
            Ok(())
        }
        "rename-route" => {
            let old = rest
                .first()
                .context("usage: project rename-route <old> <new>")?;
            let new = rest
                .get(1)
                .context("usage: project rename-route <old> <new>")?;
            validate_route(new)?;
            ensure_route_free(store, new)?;
            let mut project = store
                .project_by_route(old)?
                .with_context(|| format!("unknown project route '{old}'"))?;
            project.route = new.clone();
            store.save_project(&project)?;
            println!("renamed project route {old} -> {new}");
            Ok(())
        }
        "current" => {
            let project = project(store, globals)?;
            if globals.json {
                println!("{}", serde_json::to_string_pretty(&project_json(&project))?);
            } else {
                println!("{}", project.route);
            }
            Ok(())
        }
        "migrate" => {
            let path = rest
                .first()
                .context("usage: project migrate <path> --route <route>")?;
            let mut route = None;
            let mut i = 1;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--route")? {
                    route = Some(value);
                } else {
                    bail!("unknown project migrate option: {}", rest[i]);
                }
            }
            let route = route.context("project migrate requires --route <route>")?;
            validate_route(&route)?;
            let repo_path = resolve::validate_project_path(&resolve::expand_path(path))?;
            let project = match store.project_by_route(&route)? {
                Some(project) => project,
                None => store.add_project(
                    &route,
                    &route,
                    &repo_path,
                    "main",
                    &resolve::home_dir().join("dev").join("worktrees"),
                    &Methodology::Scrum,
                    2,
                )?,
            };
            let count = migrate_legacy(store, &project, &repo_path)?;
            println!("imported {count} legacy task(s) into {}", project.route);
            Ok(())
        }
        other => bail!("unknown project command: {other}"),
    }
}

fn ensure_route_free(store: &Store, route: &str) -> Result<()> {
    if store.project_by_route(route)?.is_some() {
        bail!("project route '{route}' already exists");
    }
    Ok(())
}

fn user_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: scrumforge user <add|list|show|set|scope|unscope|rm>");
    };
    let rest = &args[1..];
    match sub {
        "add" => user_add(store, globals, rest),
        "list" => user_list(store, globals, rest),
        "show" => user_show(store, globals, rest),
        "set" => user_set(store, globals, rest),
        "scope" => user_scope(store, globals, rest, true),
        "unscope" => user_scope(store, globals, rest, false),
        "rm" => user_rm(store, globals, rest),
        other => bail!("unknown user command: {other}"),
    }
}

fn require_user(store: &Store, handle: &str) -> Result<User> {
    store
        .user_by_handle(handle)?
        .with_context(|| format!("unknown user '{handle}'"))
}

fn project_ids_for(store: &Store, routes: &[String]) -> Result<Vec<i64>> {
    let mut ids = Vec::new();
    for route in routes {
        let project = store
            .project_by_route(route)?
            .with_context(|| format!("unknown project route '{route}'"))?;
        ids.push(project.id);
    }
    Ok(ids)
}

fn user_json(store: &Store, user: &User) -> Result<Value> {
    let projects = store
        .user_projects(user.id)?
        .into_iter()
        .map(|p| p.route)
        .collect::<Vec<_>>();
    Ok(json!({
        "id": user.id,
        "handle": user.handle,
        "name": user.name,
        "kind": user.kind,
        "notes": user.notes,
        "projects": projects,
        "created_at": user.created_at,
        "updated_at": user.updated_at,
    }))
}

fn print_user(store: &Store, user: &User) {
    let scopes = store
        .user_projects(user.id)
        .map(|projects| {
            if projects.is_empty() {
                "(all projects)".to_string()
            } else {
                projects
                    .into_iter()
                    .map(|p| p.route)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        })
        .unwrap_or_else(|_| "(unknown)".into());
    println!(
        "{:<20} {:<8} {:<24} {}",
        user.handle, user.kind, user.name, scopes
    );
    if !user.notes.is_empty() {
        println!("  notes: {}", user.notes);
    }
}

fn user_add(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let handle = args
        .first()
        .context("usage: user add <handle> [--name N] [--kind human|agent] [--scope route]...")?;
    resolve::validate_handle(handle)?;
    if store.user_by_handle(handle)?.is_some() {
        bail!("user '{handle}' already exists");
    }
    let mut name = handle.clone();
    let mut kind = "human".to_string();
    let mut notes = String::new();
    let mut routes: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        if let Some(value) = value_flag(args, &mut i, "--name")? {
            name = value;
        } else if let Some(value) = value_flag(args, &mut i, "--kind")? {
            kind = resolve::parse_user_kind(&value)?;
        } else if let Some(value) = value_flag(args, &mut i, "--notes")? {
            notes = value;
        } else if let Some(value) = value_flag(args, &mut i, "--scope")? {
            routes.push(value);
        } else {
            bail!("unknown user add option: {}", args[i]);
        }
    }
    let project_ids = project_ids_for(store, &routes)?;
    let user = store.add_user(handle, &name, &kind, &notes)?;
    if !project_ids.is_empty() {
        store.set_user_projects(user.id, &project_ids)?;
    }
    if globals.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&user_json(store, &user)?)?
        );
    } else {
        println!("added user {} ({})", user.handle, user.kind);
        print_user(store, &user);
    }
    Ok(())
}

fn user_list(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let mut project = None;
    let mut i = 0;
    while i < args.len() {
        if let Some(value) = value_flag(args, &mut i, "--scope")? {
            project = Some(value);
        } else {
            bail!("unknown user list option: {}", args[i]);
        }
    }
    let project_id = match project {
        Some(route) => Some(
            store
                .project_by_route(&route)?
                .with_context(|| format!("unknown project route '{route}'"))?
                .id,
        ),
        None => None,
    };
    let mut users = Vec::new();
    for user in store.users()? {
        if let Some(project_id) = project_id
            && !store.user_can_access_project(user.id, project_id)?
        {
            continue;
        }
        users.push(user);
    }
    if globals.json {
        let values = users
            .iter()
            .map(|u| user_json(store, u))
            .collect::<Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&values)?);
        return Ok(());
    }
    if users.is_empty() {
        println!("(no users)");
        return Ok(());
    }
    println!("{:<20} {:<8} {:<24} PROJECTS", "HANDLE", "KIND", "NAME");
    for user in &users {
        print_user(store, user);
    }
    Ok(())
}

fn user_show(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let handle = args.first().context("usage: user show <handle>")?;
    let user = require_user(store, handle)?;
    if globals.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&user_json(store, &user)?)?
        );
        return Ok(());
    }
    println!("handle: {}", user.handle);
    println!("name:   {}", user.name);
    println!("kind:   {}", user.kind);
    if !user.notes.is_empty() {
        println!("notes:  {}", user.notes);
    }
    let projects = store.user_projects(user.id)?;
    if projects.is_empty() {
        println!("scope:  all projects (unrestricted)");
    } else {
        println!(
            "scope:  {}",
            projects
                .into_iter()
                .map(|p| p.route)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    Ok(())
}

fn user_set(store: &Store, _globals: &Globals, args: &[String]) -> Result<()> {
    let handle = args
        .first()
        .context("usage: user set <handle> [--name N] [--kind human|agent] [--notes TEXT]")?;
    let mut user = require_user(store, handle)?;
    let mut i = 1;
    while i < args.len() {
        if let Some(value) = value_flag(args, &mut i, "--name")? {
            user.name = value;
        } else if let Some(value) = value_flag(args, &mut i, "--kind")? {
            user.kind = resolve::parse_user_kind(&value)?;
        } else if let Some(value) = value_flag(args, &mut i, "--notes")? {
            user.notes = value;
        } else {
            bail!("unknown user set option: {}", args[i]);
        }
    }
    store.save_user(&user)?;
    println!("updated user {}", user.handle);
    Ok(())
}

fn user_scope(store: &Store, _globals: &Globals, args: &[String], add: bool) -> Result<()> {
    let verb = if add { "scope" } else { "unscope" };
    let handle = args
        .first()
        .with_context(|| format!("usage: user {verb} <handle> <route>..."))?;
    let user = require_user(store, handle)?;
    let rest = &args[1..];
    if add && rest.iter().any(|a| a == "--clear") {
        store.set_user_projects(user.id, &[])?;
        println!(
            "user {} is now unscoped (assignable to all projects)",
            user.handle
        );
        return Ok(());
    }
    let mut routes = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        if let Some(value) = value_flag(rest, &mut i, "--scope")? {
            routes.push(value);
        } else {
            routes.push(rest[i].clone());
            i += 1;
        }
    }
    if routes.is_empty() {
        bail!("usage: user {verb} <handle> <route>...");
    }
    for route in &routes {
        let project = store
            .project_by_route(route)?
            .with_context(|| format!("unknown project route '{route}'"))?;
        if add {
            store.add_user_project(user.id, project.id)?;
        } else {
            store.remove_user_project(user.id, project.id)?;
        }
    }
    println!("user {} scopes updated:", user.handle);
    print_user(store, &user);
    Ok(())
}

fn user_rm(store: &Store, _globals: &Globals, args: &[String]) -> Result<()> {
    let handle = args.first().context("usage: user rm <handle> --yes")?;
    if !args.iter().any(|a| a == "--yes") {
        bail!("refusing to delete user without --yes");
    }
    let user = require_user(store, handle)?;
    store.remove_user(user.id)?;
    println!("deleted user {}", user.handle);
    Ok(())
}

fn project_json(p: &Project) -> Value {
    json!({
        "id": p.id,
        "route": p.route,
        "name": p.name,
        "repo_path": p.repo_path.to_string_lossy(),
        "default_branch": p.default_branch,
        "worktree_root": p.worktree_root.to_string_lossy(),
        "methodology": p.methodology.as_str(),
        "max_parallel_runs": p.max_parallel_runs,
        "created_at": p.created_at,
        "updated_at": p.updated_at,
    })
}

fn migrate_legacy(store: &Store, project: &Project, repo_path: &Path) -> Result<u32> {
    let db = repo_path.join(".scrumforge.db");
    if !db.exists() {
        bail!("no legacy database at {}", db.display());
    }
    let conn = rusqlite::Connection::open(&db)?;
    let mut count = 0u32;
    let mut imported_sprint = None;
    let mut stmt = conn.prepare(
        "SELECT id, title, description, status, assignee, branch, pr_url, review_notes FROM tasks ORDER BY id",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, Option<String>>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;
    for row in rows {
        let (_old_id, title, description, status_raw, assignee, branch, pr_url, notes) = row?;
        let status = Status::from_db(&status_raw).unwrap_or(Status::Backlog);
        let sprint_id = if status == Status::Backlog {
            None
        } else {
            if imported_sprint.is_none() {
                let sprint = store.add_sprint(
                    project.id,
                    "Imported",
                    "Legacy board import",
                    None,
                    None,
                    None,
                )?;
                let mut sprint = sprint;
                sprint.status = SprintStatus::Completed;
                store.save_sprint(&sprint)?;
                imported_sprint = Some(sprint.id);
            }
            imported_sprint
        };
        let task = store.add_task(
            project.id,
            &title,
            &description,
            "",
            &status,
            assignee.as_deref(),
            sprint_id,
            None,
            None,
            None,
            None,
        )?;
        store.update_task(project.id, task.id, |t| {
            t.branch = branch.clone();
            t.pr_url = pr_url.clone();
            t.review_notes = notes.clone();
        })?;
        store.log_event(
            project.id,
            sprint_id,
            Some(task.id),
            "migration",
            "imported",
            None,
            Some(status.as_str()),
            Some("legacy .scrumforge.db"),
        )?;
        count += 1;
    }
    Ok(count)
}

fn sprint_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: scrumforge sprint <add|list|show|activate|close|board>");
    };
    let rest = &args[1..];
    if sub == "list" && globals.all {
        let mut projects = store.projects()?;
        projects.sort_by(|a, b| a.route.cmp(&b.route));
        for project in projects {
            for sprint in store.sprints(project.id)? {
                print_sprint(&project, &sprint);
            }
        }
        return Ok(());
    }
    let project = project(store, globals)?;
    let actor = actor(globals);
    match sub {
        "add" => {
            let mut name = None;
            let mut goal = String::new();
            let mut start = None;
            let mut end = None;
            let mut capacity = None;
            let mut i = 0;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--name")? {
                    name = Some(value);
                } else if let Some(value) = value_flag(rest, &mut i, "--goal")? {
                    goal = value;
                } else if let Some(value) = value_flag(rest, &mut i, "--start")? {
                    start = Some(value);
                } else if let Some(value) = value_flag(rest, &mut i, "--end")? {
                    end = Some(value);
                } else if let Some(value) = value_flag(rest, &mut i, "--capacity")? {
                    capacity = Some(value.parse().context("invalid capacity")?);
                } else {
                    bail!("unknown sprint add option: {}", rest[i]);
                }
            }
            let next = store.sprints(project.id)?.len() + 1;
            let name = name.unwrap_or_else(|| format!("Sprint {next}"));
            let sprint = store.add_sprint(
                project.id,
                &name,
                &goal,
                start.as_deref(),
                end.as_deref(),
                capacity,
            )?;
            println!("created sprint {} — {}", sprint.number, sprint.name);
            Ok(())
        }
        "show" => {
            let sprint = find_sprint(store, &project, rest.first().map(String::as_str))?;
            if globals.json {
                println!("{}", serde_json::to_string_pretty(&sprint_json(&sprint))?);
            } else {
                print_sprint(&project, &sprint);
            }
            Ok(())
        }
        "activate" => {
            let number = parse_number(rest.first())?;
            let sprint = store.activate_sprint(project.id, number)?;
            store.log_event(
                project.id,
                Some(sprint.id),
                None,
                &actor,
                "sprint-activated",
                Some("planned"),
                Some("active"),
                Some(&sprint.name),
            )?;
            println!("activated sprint {} — {}", sprint.number, sprint.name);
            Ok(())
        }
        "close" => {
            let number = parse_number(rest.first())?;
            let mut move_to = None;
            let mut i = 1;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--move-unfinished")? {
                    move_to = Some(value);
                } else {
                    bail!("unknown sprint close option: {}", rest[i]);
                }
            }
            let sprint = store.close_sprint(project.id, number, move_to.as_deref())?;
            store.log_event(
                project.id,
                Some(sprint.id),
                None,
                &actor,
                "sprint-closed",
                Some("active"),
                Some("completed"),
                move_to.as_deref(),
            )?;
            println!("closed sprint {} — {}", sprint.number, sprint.name);
            Ok(())
        }
        "board" => {
            let sprint = find_sprint(store, &project, rest.first().map(String::as_str))?;
            let tasks = store.sprint_tasks(project.id, sprint.id)?;
            if globals.json {
                let values = tasks
                    .iter()
                    .map(|t| ops::task_json(store, t))
                    .collect::<Result<Vec<_>>>()?;
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                println!(
                    "Sprint {} — {} [{}]",
                    sprint.number, sprint.name, sprint.status
                );
                for task in &tasks {
                    agents::print_task(task);
                }
                if tasks.is_empty() {
                    println!("(sprint board is empty)");
                }
            }
            Ok(())
        }
        other => bail!("unknown sprint command: {other}"),
    }
}

fn find_sprint(
    store: &Store,
    project: &Project,
    value: Option<&str>,
) -> Result<crate::board::Sprint> {
    let value = value.unwrap_or("current");
    match value {
        "current" => store
            .active_sprint(project.id)?
            .context("project has no active sprint"),
        "next" => {
            let sprints = store.sprints(project.id)?;
            sprints
                .into_iter()
                .find(|s| s.status == SprintStatus::Planned)
                .context("project has no planned sprint")
        }
        other => {
            let number = other
                .parse::<u32>()
                .context("sprint number must be numeric")?;
            store
                .sprint_by_number(project.id, number)?
                .with_context(|| format!("sprint {number} not found"))
        }
    }
}

fn parse_number(value: Option<&String>) -> Result<u32> {
    value
        .context("missing sprint number")?
        .parse()
        .context("sprint number must be numeric")
}

fn print_sprint(project: &Project, sprint: &crate::board::Sprint) {
    println!(
        "{:<16} sprint {:<3} {:<12} {:<10} {}",
        project.route, sprint.number, sprint.status, sprint.name, sprint.goal
    );
}

fn sprint_json(s: &crate::board::Sprint) -> Value {
    json!({
        "id": s.id,
        "number": s.number,
        "name": s.name,
        "goal": s.goal,
        "status": s.status.as_str(),
        "starts_at": s.starts_at,
        "ends_at": s.ends_at,
        "capacity": s.capacity,
        "review_notes": s.review_notes,
        "retro_notes": s.retro_notes,
    })
}

fn backlog_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let project = project(store, globals)?;
    match args.first().map(String::as_str) {
        Some("list") => {
            let tasks = store.backlog_tasks(project.id)?;
            if globals.json {
                let values = tasks
                    .iter()
                    .map(|t| ops::task_json(store, t))
                    .collect::<Result<Vec<_>>>()?;
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                for task in &tasks {
                    agents::print_task(task);
                }
                if tasks.is_empty() {
                    println!("(backlog is empty)");
                }
            }
            Ok(())
        }
        Some("add") => add_backlog(store, globals, &project, &args[1..]),
        _ => add_backlog(store, globals, &project, args),
    }
}

fn add_backlog(store: &Store, globals: &Globals, project: &Project, args: &[String]) -> Result<()> {
    let mut parts = Vec::new();
    let mut sprint = None;
    let mut i = 0;
    while i < args.len() {
        if let Some(value) = value_flag(args, &mut i, "--sprint")? {
            sprint = Some(value.parse::<u32>().context("invalid sprint number")?);
        } else {
            parts.push(args[i].clone());
            i += 1;
        }
    }
    let text = parts.join(" ");
    println!(
        "{}",
        ops::add_backlog_task(store, project, &text, sprint, &actor(globals))?
    );
    Ok(())
}

fn tasks_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    if globals.all {
        for project in store.projects()? {
            println!("[{}]", project.route);
            for task in store.tasks(project.id)? {
                agents::print_task(&task);
            }
        }
        return Ok(());
    }
    let project = project(store, globals)?;
    let mut scope = "default".to_string();
    let mut i = 0;
    while i < args.len() {
        if let Some(value) = value_flag(args, &mut i, "--sprint")? {
            scope = value;
        } else {
            bail!("unknown tasks option: {}", args[i]);
        }
    }
    let tasks = match scope.as_str() {
        "backlog" => store.backlog_tasks(project.id)?,
        "all" => store.tasks(project.id)?,
        "default" | "current" | "active" => match store.active_sprint(project.id)? {
            Some(sprint) => store.sprint_tasks(project.id, sprint.id)?,
            None => store.backlog_tasks(project.id)?,
        },
        other => {
            let number = other.parse::<u32>().context("invalid sprint number")?;
            let sprint = store
                .sprint_by_number(project.id, number)?
                .with_context(|| format!("sprint {number} not found"))?;
            store.sprint_tasks(project.id, sprint.id)?
        }
    };
    if globals.json {
        let values = tasks
            .iter()
            .map(|t| ops::task_json(store, t))
            .collect::<Result<Vec<_>>>()?;
        println!("{}", serde_json::to_string_pretty(&values)?);
    } else {
        if tasks.is_empty() {
            println!("(no tasks)");
        }
        for task in &tasks {
            agents::print_task(task);
        }
    }
    Ok(())
}

fn show_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let project = project(store, globals)?;
    let id = parse_id(args.first())?;
    let task = store
        .task(project.id, id)?
        .with_context(|| format!("task #{id} not found"))?;
    if globals.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&ops::task_json(store, &task)?)?
        );
        return Ok(());
    }
    println!("#{} [{}] {}", task.id, task.status, task.title);
    if !task.description.is_empty() {
        println!("description: {}", task.description);
    }
    if !task.acceptance_criteria.is_empty() {
        println!("acceptance: {}", task.acceptance_criteria);
    }
    println!(
        "assignee: {}",
        task.assignee.as_deref().unwrap_or("unassigned")
    );
    if let Some(sprint_id) = task.sprint_id
        && let Some(sprint) = store
            .sprints(project.id)?
            .into_iter()
            .find(|s| s.id == sprint_id)
    {
        println!("sprint: {} — {}", sprint.number, sprint.name);
    }
    if let Some(spec) = &task.spec_slug {
        println!("spec: {spec}");
    }
    if let Some(key) = &task.requirement_key {
        println!("requirement: {key}");
    }
    if let Some(b) = &task.branch {
        println!("branch: {b}");
    }
    if let Some(u) = &task.pr_url {
        println!("PR: {u}");
    }
    if let Some(n) = task.review_notes.as_deref().filter(|n| !n.is_empty()) {
        println!("review: {n}");
    }
    Ok(())
}

fn spec_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: scrumforge spec <new|list|show|approve|plan|export|archive>");
    };
    let rest = &args[1..];
    let project = project(store, globals)?;
    let actor = actor(globals);
    match sub {
        "new" => {
            let slug = rest
                .first()
                .context("usage: spec new <slug> --title <title>")?;
            let mut title = None;
            let mut from = String::new();
            let mut i = 1;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--title")? {
                    title = Some(value);
                } else if let Some(value) = value_flag(rest, &mut i, "--from")? {
                    from = value;
                } else {
                    bail!("unknown spec new option: {}", rest[i]);
                }
            }
            let title = title.unwrap_or_else(|| slug.clone());
            let spec = ops::create_spec(store, &project, slug, &title, &from, &actor)?;
            println!("created spec '{}': {}", spec.slug, spec.title);
            Ok(())
        }
        "list" => {
            let mut status = None;
            let mut i = 0;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--status")? {
                    status = Some(value);
                } else {
                    bail!("unknown spec list option: {}", rest[i]);
                }
            }
            let specs = store
                .specs(project.id)?
                .into_iter()
                .filter(|s| status.as_ref().is_none_or(|want| &s.status == want))
                .collect::<Vec<_>>();
            if globals.json {
                let values = specs.iter().map(spec_json).collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                for spec in &specs {
                    println!(
                        "{:<28} {:<12} {:<5} {}",
                        spec.slug,
                        spec.status,
                        spec.requirements.len(),
                        spec.title
                    );
                }
            }
            Ok(())
        }
        "show" => {
            let slug = rest.first().context("usage: spec show <slug>")?;
            let spec = store
                .spec(project.id, slug)?
                .with_context(|| format!("spec '{slug}' not found"))?;
            if globals.json {
                println!("{}", serde_json::to_string_pretty(&spec_json(&spec))?);
            } else {
                println!("# {} ({})", spec.title, spec.status);
                println!("slug: {}", spec.slug);
                if !spec.summary.is_empty() {
                    println!("summary: {}", spec.summary);
                }
                if !spec.body_md.is_empty() {
                    println!("\n{}", spec.body_md);
                }
                if !spec.plan_md.is_empty() {
                    println!("\nPlan:\n{}", spec.plan_md);
                }
                if !spec.requirements.is_empty() {
                    println!("\nRequirements:");
                    for req in &spec.requirements {
                        println!("  {} [{}] {}", req.key, req.kind, req.text);
                    }
                }
            }
            Ok(())
        }
        "approve" => {
            let slug = rest.first().context("usage: spec approve <slug>")?;
            println!("{}", ops::approve_spec(store, &project, slug, &actor)?);
            Ok(())
        }
        "plan" => {
            let slug = rest.first().context("usage: spec plan <slug>")?;
            for line in ops::plan_spec(store, &project, slug, &actor)? {
                println!("{line}");
            }
            Ok(())
        }
        "export" => {
            let slug = rest.first().context("usage: spec export <slug>")?;
            println!("{}", ops::export_spec(store, &project, slug)?);
            Ok(())
        }
        "archive" => {
            let slug = rest.first().context("usage: spec archive <slug>")?;
            println!("{}", ops::archive_spec(store, &project, slug, &actor)?);
            Ok(())
        }
        other => bail!("unknown spec command: {other}"),
    }
}

fn spec_json(spec: &Spec) -> Value {
    json!({
        "slug": spec.slug,
        "title": spec.title,
        "status": spec.status,
        "summary": spec.summary,
        "body_md": spec.body_md,
        "plan_md": spec.plan_md,
        "requirements": spec.requirements.iter().map(|r| json!({
            "key": r.key, "text": r.text, "kind": r.kind
        })).collect::<Vec<_>>(),
        "created_by": spec.created_by,
        "created_at": spec.created_at,
        "updated_at": spec.updated_at,
    })
}

fn change_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let Some(sub) = args.first().map(String::as_str) else {
        bail!("usage: scrumforge change <new|list|approve>");
    };
    let rest = &args[1..];
    let project = project(store, globals)?;
    let actor = actor(globals);
    match sub {
        "new" => {
            let slug = rest
                .first()
                .context("usage: change new <slug> --spec <spec> --title <title>")?;
            let mut spec = None;
            let mut title = None;
            let mut body = String::new();
            let mut i = 1;
            while i < rest.len() {
                if let Some(value) = value_flag(rest, &mut i, "--spec")? {
                    spec = Some(value);
                } else if let Some(value) = value_flag(rest, &mut i, "--title")? {
                    title = Some(value);
                } else if let Some(value) = value_flag(rest, &mut i, "--body")? {
                    body = value;
                } else {
                    bail!("unknown change new option: {}", rest[i]);
                }
            }
            let spec = spec.context("change new requires --spec <spec-slug>")?;
            let title = title.unwrap_or_else(|| slug.clone());
            println!(
                "{}",
                ops::create_change(store, &project, &spec, slug, &title, &body, &actor)?
            );
            Ok(())
        }
        "list" => {
            let changes = store.changes(project.id)?;
            if globals.json {
                let values = changes.iter().map(change_json).collect::<Vec<_>>();
                println!("{}", serde_json::to_string_pretty(&values)?);
            } else {
                for change in &changes {
                    println!(
                        "{:<28} {:<12} {:<20} {}",
                        change.slug, change.status, change.spec_slug, change.title
                    );
                }
            }
            Ok(())
        }
        "approve" => {
            let slug = rest.first().context("usage: change approve <slug>")?;
            println!("{}", ops::approve_change(store, &project, slug, &actor)?);
            Ok(())
        }
        other => bail!("unknown change command: {other}"),
    }
}

fn change_json(change: &crate::board::Change) -> Value {
    json!({
        "slug": change.slug,
        "spec_slug": change.spec_slug,
        "title": change.title,
        "status": change.status,
        "body_md": change.body_md,
        "created_at": change.created_at,
        "updated_at": change.updated_at,
    })
}

fn context_cmd(store: &Store, globals: &Globals, _args: &[String]) -> Result<()> {
    let project = project(store, globals)?;
    let profile = actor(globals);
    let sprint = store.active_sprint(project.id)?;
    if globals.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "project": project_json(&project),
                "profile": profile,
                "active_sprint": sprint.as_ref().map(sprint_json),
            }))?
        );
    } else {
        println!("project:   {}", project.route);
        println!("path:      {}", project.repo_path.display());
        println!("profile:   {profile}");
        println!("method:    {}", project.methodology);
        println!("branch:    {}", project.default_branch);
        println!("worktrees: {}", project.worktree_root.display());
        if let Some(sprint) = sprint {
            println!("sprint:    {} — {}", sprint.number, sprint.name);
        }
    }
    Ok(())
}

fn activity_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let project = project(store, globals)?;
    let mut limit = 50u32;
    let mut i = 0;
    while i < args.len() {
        if let Some(value) = value_flag(args, &mut i, "--limit")? {
            limit = value.parse().context("invalid limit")?;
        } else {
            bail!("unknown activity option: {}", args[i]);
        }
    }
    let events = store.events(project.id, limit)?;
    if globals.json {
        let values = events.iter().map(event_json).collect::<Vec<_>>();
        println!("{}", serde_json::to_string_pretty(&values)?);
        return Ok(());
    }
    for event in &events {
        println!(
            "{:<19} {:<16} {:<18} {:<20} {}",
            event.ts,
            event.actor,
            event.verb,
            event
                .task_id
                .map(|id| format!("#{id}"))
                .unwrap_or_else(|| "-".into()),
            event.detail.as_deref().unwrap_or("")
        );
    }
    Ok(())
}

fn event_json(event: &crate::board::Event) -> Value {
    json!({
        "id": event.id,
        "actor": event.actor,
        "verb": event.verb,
        "task_id": event.task_id,
        "sprint_id": event.sprint_id,
        "from_status": event.from_status,
        "to_status": event.to_status,
        "detail": event.detail,
        "ts": event.ts,
    })
}

fn worktree_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    let project = project(store, globals)?;
    match args.first().map(String::as_str) {
        Some("list") => {
            for task in store.tasks(project.id)? {
                if let Some(branch) = task.branch {
                    let path = agents::worktree_path(&project, &branch);
                    println!(
                        "#{:<4} {:<12} {}",
                        task.id,
                        if path.exists() { "exists" } else { "missing" },
                        path.display()
                    );
                }
            }
            Ok(())
        }
        Some("prune") => {
            let mut removed = 0;
            for task in store.tasks(project.id)? {
                if let Some(branch) = task.branch {
                    let path = agents::worktree_path(&project, &branch);
                    if !path.exists() && task.status == Status::Done {
                        store.update_task(project.id, task.id, |t| t.branch = None)?;
                        removed += 1;
                    }
                }
            }
            println!("pruned {removed} stale worktree reference(s)");
            Ok(())
        }
        _ => bail!("usage: scrumforge worktree <list|prune>"),
    }
}

fn db_cmd(store: &Store, globals: &Globals, args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("check") => {
            let result: String = store_check(store)?;
            println!("{result}");
            Ok(())
        }
        Some("backup") => {
            let mut out = None;
            let mut i = 1;
            while i < args.len() {
                if let Some(value) = value_flag(args, &mut i, "--out")? {
                    out = Some(resolve::expand_path(&value));
                } else {
                    bail!("unknown db backup option: {}", args[i]);
                }
            }
            let out = out.unwrap_or_else(|| {
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                store
                    .db_path
                    .with_file_name(format!("scrumforge-{stamp}.db"))
            });
            std::fs::copy(&store.db_path, &out)?;
            if globals.json {
                println!("{}", json!({"backup": out.to_string_lossy()}));
            } else {
                println!("backed up {} to {}", store.db_path.display(), out.display());
            }
            Ok(())
        }
        _ => bail!("usage: scrumforge db <check|backup>"),
    }
}

fn store_check(store: &Store) -> Result<String> {
    let projects = store.projects()?;
    Ok(format!(
        "ok: {} project(s) in {}",
        projects.len(),
        store.db_path.display()
    ))
}
