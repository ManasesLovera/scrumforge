use anyhow::{Context, Result, bail};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use crate::board::{Methodology, Project, Store};

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn expand_path(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        home_dir().join(rest)
    } else if value == "~" {
        home_dir()
    } else {
        PathBuf::from(value)
    }
}

pub fn config_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(dir).join("scrumforge").join("config.toml")
    } else {
        home_dir()
            .join(".config")
            .join("scrumforge")
            .join("config.toml")
    }
}

pub fn default_db_path() -> PathBuf {
    if let Some(value) = std::env::var_os("SCRUMFORGE_DB") {
        return expand_path(&value.to_string_lossy());
    }
    if let Some(value) = config_lookup("database", "path") {
        return expand_path(&value);
    }
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(dir).join("scrumforge").join("scrumforge.db");
    }
    home_dir()
        .join(".local")
        .join("share")
        .join("scrumforge")
        .join("scrumforge.db")
}

pub fn config_lookup(section: &str, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    let mut current = "";
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && line.ends_with(']') {
            current = line[1..line.len() - 1].trim();
            continue;
        }
        if current == section
            && let Some((k, v)) = line.split_once('=')
            && k.trim() == key
        {
            return Some(unquote(v.trim()));
        }
    }
    None
}

fn unquote(value: &str) -> String {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value)
        .to_string()
}

pub fn validate_route(route: &str) -> Result<()> {
    if route.len() < 2 || route.len() > 48 {
        bail!("route must be 2-48 characters");
    }
    if !route
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("route may contain only lowercase letters, digits, and '-'");
    }
    if !route
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        || !route
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        bail!("route must start and end with a letter or digit");
    }
    const RESERVED: &[&str] = &[
        "api", "assets", "static", "p", "admin", "health", "events", "projects", "new",
    ];
    if RESERVED.contains(&route) || route.starts_with('_') {
        bail!("route '{route}' is reserved");
    }
    Ok(())
}

pub fn validate_handle(handle: &str) -> Result<()> {
    if handle.is_empty() || handle.len() > 48 {
        bail!("user handle must be 1-48 characters");
    }
    if !handle
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        bail!("user handle may contain only lowercase letters, digits, '-' and '_'");
    }
    if !handle
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        || !handle
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        bail!("user handle must start and end with a letter or digit");
    }
    Ok(())
}

pub fn parse_user_kind(value: &str) -> Result<String> {
    match value {
        "human" | "agent" => Ok(value.to_string()),
        other => bail!("unknown user kind '{other}' (expected human or agent)"),
    }
}

#[derive(Debug, Clone, Default)]
pub struct Binding {
    pub route: Option<String>,
    pub default_profile: Option<String>,
}

pub fn binding_for_cwd() -> Binding {
    let Ok(mut dir) = std::env::current_dir() else {
        return Binding::default();
    };
    loop {
        let file = dir.join(".scrumforge.toml");
        if let Ok(text) = std::fs::read_to_string(&file) {
            let mut binding = Binding::default();
            for line in text.lines() {
                let line = line.split('#').next().unwrap_or("").trim();
                let Some((key, value)) = line.split_once('=') else {
                    continue;
                };
                let value = unquote(value.trim());
                match key.trim() {
                    "route" | "project" => binding.route = Some(value),
                    "default_profile" => binding.default_profile = Some(value),
                    _ => {}
                }
            }
            return binding;
        }
        if !dir.pop() {
            return Binding::default();
        }
    }
}

pub fn resolve_explicit(store: &Store, explicit: Option<&str>) -> Result<Option<Project>> {
    let binding = binding_for_cwd();
    let route = explicit
        .map(str::to_string)
        .or_else(|| std::env::var("SCRUMFORGE_PROJECT").ok())
        .or(binding.route);
    if let Some(route) = route {
        return store
            .project_by_route(&route)?
            .with_context(|| format!("unknown project route '{route}'"))
            .map(Some);
    }

    let Ok(cwd) = std::env::current_dir() else {
        return Ok(None);
    };
    let cwd = cwd.canonicalize().unwrap_or(cwd);
    let mut best: Option<(usize, Project)> = None;
    for project in store.projects()? {
        let mut roots = vec![project.repo_path.clone()];
        roots.push(project.worktree_root.join(&project.route));
        for root in roots {
            let root = root.canonicalize().unwrap_or(root);
            if cwd.starts_with(&root) {
                let len = root.components().count();
                if best.as_ref().is_none_or(|(best_len, _)| len > *best_len) {
                    best = Some((len, project.clone()));
                }
            }
        }
    }
    Ok(best.map(|(_, project)| project))
}

pub fn resolve_project(store: &Store, explicit: Option<&str>, allow_pick: bool) -> Result<Project> {
    if let Some(project) = resolve_explicit(store, explicit)? {
        return Ok(project);
    }
    let projects = store.projects()?;
    if projects.is_empty() {
        bail!("no projects registered; run: scrumforge project add <route> <path>");
    }
    if allow_pick && std::io::stdin().is_terminal() {
        eprintln!("Select a project:");
        for (i, p) in projects.iter().enumerate() {
            eprintln!("  {}. {} ({})", i + 1, p.route, p.name);
        }
        eprint!("project> ");
        std::io::stderr().flush().ok();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let choice = line.trim();
        let project = choice
            .parse::<usize>()
            .ok()
            .and_then(|n| projects.get(n.saturating_sub(1)))
            .or_else(|| projects.iter().find(|p| p.route == choice));
        if let Some(project) = project {
            return Ok(project.clone());
        }
        bail!("no project selected");
    }
    let routes = projects
        .iter()
        .map(|p| p.route.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!("no project resolved; pass --project <route> (known: {routes})")
}

pub fn resolve_profile(explicit: Option<&str>) -> String {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("SCRUMFORGE_PROFILE").ok())
        .or_else(|| binding_for_cwd().default_profile)
        .or_else(|| config_lookup("defaults", "default_profile"))
        .unwrap_or_else(|| "product-owner".to_string())
}

pub fn parse_methodology(value: &str) -> Result<Methodology> {
    match value {
        "scrum" => Ok(Methodology::Scrum),
        "sdd" => Ok(Methodology::Sdd),
        other => bail!("unknown methodology '{other}' (expected scrum or sdd)"),
    }
}

pub fn validate_project_path(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .with_context(|| format!("project path does not exist: {}", path.display()))?;
    if !path.join(".git").exists() {
        bail!("project path is not a git repository: {}", path.display());
    }
    Ok(path)
}
