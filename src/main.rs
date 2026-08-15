mod agents;
mod board;
mod cli;
mod ops;
mod repl;
mod tui;

use anyhow::{bail, Result};
use board::Board;
use std::path::PathBuf;

fn find_repo_root() -> Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        if dir.join(".git").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            bail!(
                "not inside a git repository (looked up from {})",
                std::env::current_dir()?.display()
            );
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("-h") | Some("--help") | Some("help") => {
            println!("{}", cli::HELP);
            Ok(())
        }
        Some("-V") | Some("--version") => {
            println!("scrumforge {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => {
            let repo_path = find_repo_root()?;
            let board_file: PathBuf = repo_path.join(".scrumforge.db");
            let board = Board::open(&board_file, repo_path.clone())?;
            match args.first().map(String::as_str) {
                None | Some("tui") => tui::run(board),
                Some("repl") => repl::run(board),
                Some(cmd) => cli::run(board, cmd, &args[1..]),
            }
        }
    }
}
