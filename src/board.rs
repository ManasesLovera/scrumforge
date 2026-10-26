use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Backlog,
    Todo,
    InProgress,
    InReview,
    ChangesRequested,
    Done,
    Cancelled,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Todo => "todo",
            Status::InProgress => "in-progress",
            Status::InReview => "in-review",
            Status::ChangesRequested => "changes-requested",
            Status::Done => "done",
            Status::Cancelled => "cancelled",
        }
    }

    pub fn from_db(s: &str) -> Result<Status> {
        Ok(match s {
            "backlog" => Status::Backlog,
            "todo" | "assigned" => Status::Todo,
            "in-progress" => Status::InProgress,
            "in-review" => Status::InReview,
            "changes-requested" => Status::ChangesRequested,
            "done" => Status::Done,
            "cancelled" => Status::Cancelled,
            other => bail!("unknown status in db: {other}"),
        })
    }

    pub fn allowed(from: &Status, to: &Status) -> bool {
        use Status::*;
        from == to
            || matches!(
                (from, to),
                (Backlog, Todo)
                    | (Todo, Backlog | InProgress)
                    | (InProgress, Todo | InReview)
                    | (InReview, InProgress | ChangesRequested | Done)
                    | (ChangesRequested, InProgress | InReview)
                    | (Done, InReview)
                    | (_, Cancelled)
            )
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Methodology {
    Scrum,
    Sdd,
}

impl Methodology {
    pub fn as_str(&self) -> &'static str {
        match self {
            Methodology::Scrum => "scrum",
            Methodology::Sdd => "sdd",
        }
    }

    pub fn from_db(s: &str) -> Result<Methodology> {
        Ok(match s {
            "scrum" => Methodology::Scrum,
            "sdd" => Methodology::Sdd,
            other => bail!("unknown methodology: {other}"),
        })
    }
}

impl fmt::Display for Methodology {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SprintStatus {
    Planned,
    Active,
    Completed,
}

impl SprintStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SprintStatus::Planned => "planned",
            SprintStatus::Active => "active",
            SprintStatus::Completed => "completed",
        }
    }

    pub fn from_db(s: &str) -> Result<SprintStatus> {
        Ok(match s {
            "planned" => SprintStatus::Planned,
            "active" => SprintStatus::Active,
            "completed" => SprintStatus::Completed,
            other => bail!("unknown sprint status: {other}"),
        })
    }
}

impl fmt::Display for SprintStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Project {
    pub id: i64,
    pub route: String,
    pub name: String,
    pub repo_path: PathBuf,
    pub default_branch: String,
    pub worktree_root: PathBuf,
    pub methodology: Methodology,
    pub max_parallel_runs: u32,
    pub archived_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Sprint {
    pub id: i64,
    #[allow(dead_code)]
    pub project_id: i64,
    pub number: u32,
    pub name: String,
    pub goal: String,
    pub status: SprintStatus,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub capacity: Option<f64>,
    pub review_notes: Option<String>,
    pub retro_notes: Option<String>,
    #[allow(dead_code)]
    pub created_at: String,
    #[allow(dead_code)]
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: u32,
    #[allow(dead_code)]
    pub project_id: i64,
    pub sprint_id: Option<i64>,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: String,
    pub status: Status,
    pub assignee: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub review_notes: Option<String>,
    pub spec_slug: Option<String>,
    pub change_slug: Option<String>,
    pub requirement_key: Option<String>,
    pub estimate: Option<f64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Requirement {
    pub key: String,
    pub text: String,
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct Spec {
    pub id: i64,
    #[allow(dead_code)]
    pub project_id: i64,
    pub slug: String,
    pub title: String,
    pub status: String,
    pub summary: String,
    pub body_md: String,
    pub plan_md: String,
    pub requirements: Vec<Requirement>,
    pub created_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Change {
    #[allow(dead_code)]
    pub id: i64,
    #[allow(dead_code)]
    pub project_id: i64,
    pub spec_slug: String,
    pub slug: String,
    pub title: String,
    pub status: String,
    pub body_md: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct Event {
    #[allow(dead_code)]
    pub id: i64,
    #[allow(dead_code)]
    pub project_id: i64,
    pub sprint_id: Option<i64>,
    pub task_id: Option<u32>,
    pub actor: String,
    pub verb: String,
    pub from_status: Option<String>,
    pub to_status: Option<String>,
    pub detail: Option<String>,
    pub ts: String,
}

pub struct Store {
    conn: Connection,
    pub db_path: PathBuf,
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS projects (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    route            TEXT NOT NULL UNIQUE COLLATE NOCASE,
    name             TEXT NOT NULL,
    repo_path        TEXT NOT NULL UNIQUE,
    default_branch   TEXT NOT NULL DEFAULT 'main',
    worktree_root    TEXT NOT NULL,
    methodology      TEXT NOT NULL DEFAULT 'scrum',
    max_parallel_runs INTEGER NOT NULL DEFAULT 2,
    archived_at      TEXT,
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS sprints (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    number       INTEGER NOT NULL,
    name         TEXT NOT NULL,
    goal         TEXT NOT NULL DEFAULT '',
    status       TEXT NOT NULL DEFAULT 'planned',
    starts_at    TEXT,
    ends_at      TEXT,
    capacity     REAL,
    review_notes TEXT,
    retro_notes  TEXT,
    created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (project_id, number)
);
CREATE TABLE IF NOT EXISTS tasks (
    id                   INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id           INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sprint_id            INTEGER REFERENCES sprints(id) ON DELETE SET NULL,
    title                TEXT NOT NULL,
    description          TEXT NOT NULL DEFAULT '',
    acceptance_criteria  TEXT NOT NULL DEFAULT '',
    status               TEXT NOT NULL DEFAULT 'backlog',
    assignee             TEXT,
    branch               TEXT,
    pr_url               TEXT,
    review_notes         TEXT,
    spec_slug            TEXT,
    change_slug          TEXT,
    requirement_key      TEXT,
    estimate             REAL,
    position             REAL NOT NULL DEFAULT 0,
    created_at           TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at           TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE IF NOT EXISTS specs (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id       INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    slug             TEXT NOT NULL,
    title            TEXT NOT NULL,
    status           TEXT NOT NULL DEFAULT 'draft',
    summary          TEXT NOT NULL DEFAULT '',
    body_md          TEXT NOT NULL DEFAULT '',
    plan_md          TEXT NOT NULL DEFAULT '',
    requirements_json TEXT NOT NULL DEFAULT '[]',
    created_by       TEXT,
    created_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at       TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (project_id, slug)
);
CREATE TABLE IF NOT EXISTS changes (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    spec_slug    TEXT NOT NULL,
    slug         TEXT NOT NULL,
    title        TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'proposed',
    body_md      TEXT NOT NULL DEFAULT '',
    created_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (project_id, slug)
);
CREATE TABLE IF NOT EXISTS events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    project_id   INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    sprint_id    INTEGER,
    task_id      INTEGER,
    actor        TEXT NOT NULL DEFAULT 'product-owner',
    verb         TEXT NOT NULL,
    from_status  TEXT,
    to_status    TEXT,
    detail       TEXT,
    ts           TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_tasks_project_status ON tasks(project_id, status);
CREATE INDEX IF NOT EXISTS idx_tasks_sprint ON tasks(sprint_id);
CREATE INDEX IF NOT EXISTS idx_sprints_project ON sprints(project_id, number);
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_active_sprint
    ON sprints(project_id) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_events_project ON events(project_id, id DESC);
"#;

impl Store {
    pub fn open(db_file: &Path) -> Result<Store> {
        if let Some(parent) = db_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data directory {}", parent.display()))?;
        }
        let conn = Connection::open(db_file)
            .with_context(|| format!("opening sqlite db {}", db_file.display()))?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(SCHEMA)
            .with_context(|| format!("initializing schema in {}", db_file.display()))?;
        conn.execute(
            "UPDATE tasks SET status = 'todo' WHERE status = 'assigned'",
            [],
        )
        .context("migrating assigned status to todo")?;
        Ok(Store {
            conn,
            db_path: db_file.to_path_buf(),
        })
    }

    pub fn projects(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, route, name, repo_path, default_branch, worktree_root,
                    methodology, max_parallel_runs, archived_at, created_at, updated_at
             FROM projects WHERE archived_at IS NULL ORDER BY route",
        )?;
        let rows = stmt.query_map([], project_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing projects")
    }

    pub fn project_by_route(&self, route: &str) -> Result<Option<Project>> {
        self.conn
            .query_row(
                "SELECT id, route, name, repo_path, default_branch, worktree_root,
                        methodology, max_parallel_runs, archived_at, created_at, updated_at
                 FROM projects WHERE route = ?1 COLLATE NOCASE",
                params![route],
                project_from_row,
            )
            .optional()
            .context("looking up project")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_project(
        &self,
        route: &str,
        name: &str,
        repo_path: &Path,
        default_branch: &str,
        worktree_root: &Path,
        methodology: &Methodology,
        max_parallel_runs: u32,
    ) -> Result<Project> {
        self.conn.execute(
            "INSERT INTO projects
             (route, name, repo_path, default_branch, worktree_root, methodology, max_parallel_runs)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                route,
                name,
                repo_path.to_string_lossy(),
                default_branch,
                worktree_root.to_string_lossy(),
                methodology.as_str(),
                max_parallel_runs,
            ],
        )?;
        self.project_by_route(route)?
            .context("project missing after insert")
    }

    pub fn save_project(&self, p: &Project) -> Result<()> {
        self.conn.execute(
            "UPDATE projects SET route = ?1, name = ?2, repo_path = ?3,
                default_branch = ?4, worktree_root = ?5, methodology = ?6,
                max_parallel_runs = ?7, archived_at = ?8, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?9",
            params![
                p.route,
                p.name,
                p.repo_path.to_string_lossy(),
                p.default_branch,
                p.worktree_root.to_string_lossy(),
                p.methodology.as_str(),
                p.max_parallel_runs,
                p.archived_at,
                p.id,
            ],
        )?;
        Ok(())
    }

    pub fn remove_project(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM projects WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn sprints(&self, project_id: i64) -> Result<Vec<Sprint>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, number, name, goal, status, starts_at, ends_at,
                    capacity, review_notes, retro_notes, created_at, updated_at
             FROM sprints WHERE project_id = ?1 ORDER BY number",
        )?;
        let rows = stmt.query_map(params![project_id], sprint_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing sprints")
    }

    pub fn active_sprint(&self, project_id: i64) -> Result<Option<Sprint>> {
        self.conn
            .query_row(
                "SELECT id, project_id, number, name, goal, status, starts_at, ends_at,
                        capacity, review_notes, retro_notes, created_at, updated_at
                 FROM sprints WHERE project_id = ?1 AND status = 'active'",
                params![project_id],
                sprint_from_row,
            )
            .optional()
            .context("looking up active sprint")
    }

    pub fn sprint_by_number(&self, project_id: i64, number: u32) -> Result<Option<Sprint>> {
        self.conn
            .query_row(
                "SELECT id, project_id, number, name, goal, status, starts_at, ends_at,
                        capacity, review_notes, retro_notes, created_at, updated_at
                 FROM sprints WHERE project_id = ?1 AND number = ?2",
                params![project_id, number],
                sprint_from_row,
            )
            .optional()
            .context("looking up sprint")
    }

    pub fn add_sprint(
        &self,
        project_id: i64,
        name: &str,
        goal: &str,
        starts_at: Option<&str>,
        ends_at: Option<&str>,
        capacity: Option<f64>,
    ) -> Result<Sprint> {
        let number: u32 = self.conn.query_row(
            "SELECT COALESCE(MAX(number), 0) + 1 FROM sprints WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO sprints (project_id, number, name, goal, starts_at, ends_at, capacity)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![project_id, number, name, goal, starts_at, ends_at, capacity],
        )?;
        self.sprint_by_number(project_id, number)?
            .context("sprint missing after insert")
    }

    pub fn save_sprint(&self, s: &Sprint) -> Result<()> {
        self.conn.execute(
            "UPDATE sprints SET name = ?1, goal = ?2, status = ?3, starts_at = ?4,
                ends_at = ?5, capacity = ?6, review_notes = ?7, retro_notes = ?8,
                updated_at = CURRENT_TIMESTAMP WHERE id = ?9",
            params![
                s.name,
                s.goal,
                s.status.as_str(),
                s.starts_at,
                s.ends_at,
                s.capacity,
                s.review_notes,
                s.retro_notes,
                s.id,
            ],
        )?;
        Ok(())
    }

    pub fn activate_sprint(&self, project_id: i64, number: u32) -> Result<Sprint> {
        let sprint = self
            .sprint_by_number(project_id, number)?
            .with_context(|| format!("sprint {number} not found"))?;
        self.conn.execute(
            "UPDATE sprints SET status = 'planned', updated_at = CURRENT_TIMESTAMP
             WHERE project_id = ?1 AND status = 'active' AND id != ?2",
            params![project_id, sprint.id],
        )?;
        self.conn.execute(
            "UPDATE sprints SET status = 'active', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![sprint.id],
        )?;
        self.sprint_by_number(project_id, number)?
            .context("sprint missing after activate")
    }

    pub fn close_sprint(
        &self,
        project_id: i64,
        number: u32,
        target: Option<&str>,
    ) -> Result<Sprint> {
        let sprint = self
            .sprint_by_number(project_id, number)?
            .with_context(|| format!("sprint {number} not found"))?;
        let unfinished: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE sprint_id = ?1 AND status != 'done'",
            params![sprint.id],
            |row| row.get(0),
        )?;
        let target = if unfinished > 0 {
            Some(target.unwrap_or("backlog"))
        } else {
            target
        };
        if let Some(target) = target {
            if target == "backlog" {
                self.conn.execute(
                    "UPDATE tasks SET sprint_id = NULL, status = 'backlog', updated_at = CURRENT_TIMESTAMP
                     WHERE sprint_id = ?1 AND status != 'done'",
                    params![sprint.id],
                )?;
            } else {
                let target_number: u32 = target
                    .parse()
                    .with_context(|| format!("invalid target sprint: {target}"))?;
                let target_sprint = self
                    .sprint_by_number(project_id, target_number)?
                    .with_context(|| format!("target sprint {target_number} not found"))?;
                self.conn.execute(
                    "UPDATE tasks SET sprint_id = ?1,
                        status = CASE WHEN status = 'backlog' THEN 'todo' ELSE status END,
                        updated_at = CURRENT_TIMESTAMP
                     WHERE sprint_id = ?2 AND status != 'done'",
                    params![target_sprint.id, sprint.id],
                )?;
            }
        }
        self.conn.execute(
            "UPDATE sprints SET status = 'completed', updated_at = CURRENT_TIMESTAMP WHERE id = ?1",
            params![sprint.id],
        )?;
        self.sprint_by_number(project_id, number)?
            .context("sprint missing after close")
    }

    pub fn sprint_tasks(&self, project_id: i64, sprint_id: i64) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "{} WHERE project_id = ?1 AND sprint_id = ?2 ORDER BY position, id",
            TASK_SELECT
        ))?;
        let rows = stmt.query_map(params![project_id, sprint_id], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing sprint tasks")
    }

    pub fn backlog_tasks(&self, project_id: i64) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "{} WHERE project_id = ?1 AND sprint_id IS NULL ORDER BY position, id",
            TASK_SELECT
        ))?;
        let rows = stmt.query_map(params![project_id], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing backlog tasks")
    }

    pub fn tasks(&self, project_id: i64) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(&format!(
            "{} WHERE project_id = ?1 ORDER BY id",
            TASK_SELECT
        ))?;
        let rows = stmt.query_map(params![project_id], task_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing tasks")
    }

    pub fn task(&self, project_id: i64, id: u32) -> Result<Option<Task>> {
        self.conn
            .query_row(
                &format!("{} WHERE project_id = ?1 AND id = ?2", TASK_SELECT),
                params![project_id, id],
                task_from_row,
            )
            .optional()
            .context("looking up task")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_task(
        &self,
        project_id: i64,
        title: &str,
        description: &str,
        acceptance_criteria: &str,
        status: &Status,
        assignee: Option<&str>,
        sprint_id: Option<i64>,
        spec_slug: Option<&str>,
        change_slug: Option<&str>,
        requirement_key: Option<&str>,
        estimate: Option<f64>,
    ) -> Result<Task> {
        self.conn.execute(
            "INSERT INTO tasks
             (project_id, sprint_id, title, description, acceptance_criteria, status,
              assignee, spec_slug, change_slug, requirement_key, estimate)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                project_id,
                sprint_id,
                title,
                description,
                acceptance_criteria,
                status.as_str(),
                assignee,
                spec_slug,
                change_slug,
                requirement_key,
                estimate,
            ],
        )?;
        let id = self.conn.last_insert_rowid() as u32;
        self.task(project_id, id)?
            .context("task missing after insert")
    }

    pub fn claim_task(&self, project_id: i64, id: u32, assignee: &str) -> Result<Option<Task>> {
        let changed = self.conn.execute(
            "UPDATE tasks SET status = 'in-progress', assignee = ?1,
                updated_at = CURRENT_TIMESTAMP
             WHERE id = ?2 AND project_id = ?3 AND status IN ('todo', 'changes-requested')",
            params![assignee, id, project_id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.task(project_id, id)
    }

    pub fn save_task(&self, t: &Task) -> Result<()> {
        self.conn.execute(
            "UPDATE tasks SET sprint_id = ?1, title = ?2, description = ?3,
                acceptance_criteria = ?4, status = ?5, assignee = ?6, branch = ?7,
                pr_url = ?8, review_notes = ?9, spec_slug = ?10, change_slug = ?11,
                requirement_key = ?12, estimate = ?13, updated_at = CURRENT_TIMESTAMP
             WHERE id = ?14 AND project_id = ?15",
            params![
                t.sprint_id,
                t.title,
                t.description,
                t.acceptance_criteria,
                t.status.as_str(),
                t.assignee,
                t.branch,
                t.pr_url,
                t.review_notes,
                t.spec_slug,
                t.change_slug,
                t.requirement_key,
                t.estimate,
                t.id,
                t.project_id,
            ],
        )?;
        Ok(())
    }

    pub fn update_task(&self, project_id: i64, id: u32, f: impl FnOnce(&mut Task)) -> Result<Task> {
        let mut task = self
            .task(project_id, id)?
            .with_context(|| format!("task #{id} not found in this project"))?;
        f(&mut task);
        self.save_task(&task)?;
        Ok(task)
    }

    pub fn specs(&self, project_id: i64) -> Result<Vec<Spec>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, slug, title, status, summary, body_md, plan_md,
                    requirements_json, created_by, created_at, updated_at
             FROM specs WHERE project_id = ?1 ORDER BY slug",
        )?;
        let rows = stmt.query_map(params![project_id], spec_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing specs")
    }

    pub fn spec(&self, project_id: i64, slug: &str) -> Result<Option<Spec>> {
        self.conn
            .query_row(
                "SELECT id, project_id, slug, title, status, summary, body_md, plan_md,
                        requirements_json, created_by, created_at, updated_at
                 FROM specs WHERE project_id = ?1 AND slug = ?2",
                params![project_id, slug],
                spec_from_row,
            )
            .optional()
            .context("looking up spec")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_spec(
        &self,
        project_id: i64,
        slug: &str,
        title: &str,
        status: &str,
        summary: &str,
        body_md: &str,
        plan_md: &str,
        requirements_json: &str,
        created_by: Option<&str>,
    ) -> Result<Spec> {
        self.conn.execute(
            "INSERT INTO specs
             (project_id, slug, title, status, summary, body_md, plan_md, requirements_json, created_by)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                project_id,
                slug,
                title,
                status,
                summary,
                body_md,
                plan_md,
                requirements_json,
                created_by
            ],
        )?;
        self.spec(project_id, slug)?
            .context("spec missing after insert")
    }

    pub fn save_spec(&self, s: &Spec) -> Result<()> {
        let json = serde_json::to_string(
            &s.requirements
                .iter()
                .map(|r| serde_json::json!({"key": r.key, "text": r.text, "kind": r.kind}))
                .collect::<Vec<_>>(),
        )?;
        self.conn.execute(
            "UPDATE specs SET slug = ?1, title = ?2, status = ?3, summary = ?4,
                body_md = ?5, plan_md = ?6, requirements_json = ?7,
                updated_at = CURRENT_TIMESTAMP WHERE id = ?8",
            params![
                s.slug, s.title, s.status, s.summary, s.body_md, s.plan_md, json, s.id
            ],
        )?;
        Ok(())
    }

    pub fn changes(&self, project_id: i64) -> Result<Vec<Change>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, spec_slug, slug, title, status, body_md, created_at, updated_at
             FROM changes WHERE project_id = ?1 ORDER BY slug",
        )?;
        let rows = stmt.query_map(params![project_id], change_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing changes")
    }

    pub fn add_change(
        &self,
        project_id: i64,
        spec_slug: &str,
        slug: &str,
        title: &str,
        body_md: &str,
    ) -> Result<Change> {
        self.conn.execute(
            "INSERT INTO changes (project_id, spec_slug, slug, title, body_md)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![project_id, spec_slug, slug, title, body_md],
        )?;
        let id = self.conn.last_insert_rowid();
        self.conn
            .query_row(
                "SELECT id, project_id, spec_slug, slug, title, status, body_md, created_at, updated_at
                 FROM changes WHERE id = ?1",
                params![id],
                change_from_row,
            )
            .context("change missing after insert")
    }

    pub fn approve_change(&self, project_id: i64, slug: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE changes SET status = 'approved', updated_at = CURRENT_TIMESTAMP
             WHERE project_id = ?1 AND slug = ?2",
            params![project_id, slug],
        )?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn log_event(
        &self,
        project_id: i64,
        sprint_id: Option<i64>,
        task_id: Option<u32>,
        actor: &str,
        verb: &str,
        from_status: Option<&str>,
        to_status: Option<&str>,
        detail: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO events
             (project_id, sprint_id, task_id, actor, verb, from_status, to_status, detail)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                project_id,
                sprint_id,
                task_id,
                actor,
                verb,
                from_status,
                to_status,
                detail
            ],
        )?;
        Ok(())
    }

    pub fn events(&self, project_id: i64, limit: u32) -> Result<Vec<Event>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, project_id, sprint_id, task_id, actor, verb, from_status, to_status, detail, ts
             FROM events WHERE project_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![project_id, limit], event_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("listing events")
    }

    pub fn counts(&self, project_id: i64) -> Result<(i64, i64, i64, i64)> {
        let backlog: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE project_id = ?1 AND sprint_id IS NULL",
            params![project_id],
            |row| row.get(0),
        )?;
        let done: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tasks WHERE project_id = ?1 AND status = 'done'",
            params![project_id],
            |row| row.get(0),
        )?;
        let specs: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM specs WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;
        let sprints: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sprints WHERE project_id = ?1",
            params![project_id],
            |row| row.get(0),
        )?;
        Ok((backlog, done, specs, sprints))
    }
}

const TASK_SELECT: &str = "SELECT id, project_id, sprint_id, title, description,
    acceptance_criteria, status, assignee, branch, pr_url, review_notes,
    spec_slug, change_slug, requirement_key, estimate, created_at, updated_at
    FROM tasks";

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let methodology: String = row.get(6)?;
    Ok(Project {
        id: row.get(0)?,
        route: row.get(1)?,
        name: row.get(2)?,
        repo_path: PathBuf::from(row.get::<_, String>(3)?),
        default_branch: row.get(4)?,
        worktree_root: PathBuf::from(row.get::<_, String>(5)?),
        methodology: Methodology::from_db(&methodology).map_err(to_sql_err)?,
        max_parallel_runs: row.get::<_, u32>(7)?,
        archived_at: row.get(8)?,
        created_at: row.get(9)?,
        updated_at: row.get(10)?,
    })
}

fn sprint_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Sprint> {
    let status: String = row.get(5)?;
    Ok(Sprint {
        id: row.get(0)?,
        project_id: row.get(1)?,
        number: row.get(2)?,
        name: row.get(3)?,
        goal: row.get(4)?,
        status: SprintStatus::from_db(&status).map_err(to_sql_err)?,
        starts_at: row.get(6)?,
        ends_at: row.get(7)?,
        capacity: row.get(8)?,
        review_notes: row.get(9)?,
        retro_notes: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    let status: String = row.get(6)?;
    Ok(Task {
        id: row.get(0)?,
        project_id: row.get(1)?,
        sprint_id: row.get(2)?,
        title: row.get(3)?,
        description: row.get(4)?,
        acceptance_criteria: row.get(5)?,
        status: Status::from_db(&status).map_err(to_sql_err)?,
        assignee: row.get(7)?,
        branch: row.get(8)?,
        pr_url: row.get(9)?,
        review_notes: row.get(10)?,
        spec_slug: row.get(11)?,
        change_slug: row.get(12)?,
        requirement_key: row.get(13)?,
        estimate: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
    })
}

fn spec_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Spec> {
    let raw: String = row.get(8)?;
    let requirements = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .unwrap_or_default()
        .into_iter()
        .map(|v| Requirement {
            key: v
                .get("key")
                .and_then(|x| x.as_str())
                .unwrap_or("R")
                .to_string(),
            text: v
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            kind: v
                .get("kind")
                .and_then(|x| x.as_str())
                .unwrap_or("functional")
                .to_string(),
        })
        .collect();
    Ok(Spec {
        id: row.get(0)?,
        project_id: row.get(1)?,
        slug: row.get(2)?,
        title: row.get(3)?,
        status: row.get(4)?,
        summary: row.get(5)?,
        body_md: row.get(6)?,
        plan_md: row.get(7)?,
        requirements,
        created_by: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
    })
}

fn change_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Change> {
    Ok(Change {
        id: row.get(0)?,
        project_id: row.get(1)?,
        spec_slug: row.get(2)?,
        slug: row.get(3)?,
        title: row.get(4)?,
        status: row.get(5)?,
        body_md: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Event> {
    Ok(Event {
        id: row.get(0)?,
        project_id: row.get(1)?,
        sprint_id: row.get(2)?,
        task_id: row.get(3)?,
        actor: row.get(4)?,
        verb: row.get(5)?,
        from_status: row.get(6)?,
        to_status: row.get(7)?,
        detail: row.get(8)?,
        ts: row.get(9)?,
    })
}

fn to_sql_err(error: anyhow::Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::other(error.to_string())),
    )
}
