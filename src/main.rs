mod agents;
mod board;
mod cli;
mod help;
mod ops;
mod repl;
mod resolve;
mod serve;
mod tui;

use anyhow::{Result, bail};
use board::Store;

fn main() -> Result<()> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if matches!(raw.first().map(String::as_str), Some("-h") | Some("--help")) {
        println!("{}", cli::HELP);
        return Ok(());
    }
    if matches!(
        raw.first().map(String::as_str),
        Some("-V") | Some("--version")
    ) {
        println!("scrumforge {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if raw.first().map(String::as_str) == Some("help") {
        help::print(&raw[1..]);
        return Ok(());
    }

    let (globals, rest) = cli::parse_globals(raw)?;
    let db_path = globals.db.clone().unwrap_or_else(resolve::default_db_path);
    let store = Store::open(&db_path)?;

    match rest.first().map(String::as_str) {
        Some("help") => {
            help::print(&rest[1..]);
            Ok(())
        }
        Some("tui") => {
            let project = resolve::resolve_project(&store, globals.project.as_deref(), true)?;
            tui::run(store, project)
        }
        Some("repl") => {
            let project = resolve::resolve_project(&store, globals.project.as_deref(), true)?;
            repl::run(store, project)
        }
        Some("--web") | Some("serve") => {
            let port = serve::parse_port(&rest[1..])?;
            serve::run(store, port)
        }
        None => {
            let project = resolve::resolve_project(&store, globals.project.as_deref(), true)?;
            tui::run(store, project)
        }
        Some(_) => cli::run(&store, &globals, &rest),
    }
}

#[allow(dead_code)]
fn unreachable_bail() -> Result<()> {
    bail!("unreachable")
}
