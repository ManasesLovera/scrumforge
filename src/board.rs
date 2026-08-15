use anyhow::{Context, Result};
use rusqlite::Connection;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Backlog,
    Assigned,
    InProgress,
    InReview,
    ChangesRequested,
    Done,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Backlog => "backlog",
            Status::Assigned => "assigned",
            Status::InProgress => "in-progress",
            Status::InReview => "in-review",
            Status::ChangesRequested => "changes-requested",
            Status::Done => "done",
        }
    }

    fn from_db(s: &str) -> Result<Status> {
        Ok(match s {
            "backlog" => Status::Backlog,
            "assigned" => Status::Assigned,
            "in-progress" => Status::InProgress,
            "in-review" => Status::InReview,
            "changes-requested" => Status::ChangesRequested,
            "done" => Status::Done,
            other => anyhow::bail!("unknown status in db: {other}"),
        })
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub description: String,
    pub status: Status,
    pub assignee: Option<String>,
    pub branch: Option<String>,
    pub pr_url: Option<String>,
    pub review_notes: Option<String>,
}

#[derive(Debug)]
pub struct Board {
    pub repo_path: PathBuf,
    conn: Connection,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS tasks (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    title        TEXT NOT NULL,
    description  TEXT NOT NULL DEFAULT '',
    status       TEXT NOT NULL DEFAULT 'backlog',
    assignee     TEXT,
    branch       TEXT,
    pr_url       TEXT,
    review_notes TEXT
);
";

impl Board {
    pub fn open(db_file: &Path, repo_path: PathBuf) -> Result<Board> {
        let conn = Connection::open(db_file)
            .with_context(|| format!("opening sqlite db {}", db_file.display()))?;
        conn.execute_batch(SCHEMA)
            .with_context(|| format!("initializing schema in {}", db_file.display()))?;
        Ok(Board { repo_path, conn })
    }

    pub fn add_task(&mut self, title: &str, description: &str) -> Result<Task> {
        self.conn
            .execute(
                "INSERT INTO tasks (title, description) VALUES (?1, ?2)",
                rusqlite::params![title, description],
            )
            .context("inserting task")?;
        let id = self.conn.last_insert_rowid() as u32;
        Ok(Task {
            id,
            title: title.to_string(),
            description: description.to_string(),
            status: Status::Backlog,
            assignee: None,
            branch: None,
            pr_url: None,
            review_notes: None,
        })
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.load_tasks().unwrap_or_default()
    }

    fn load_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, description, status, assignee, branch, pr_url, review_notes FROM tasks ORDER BY id")
            .context("preparing task query")?;
        let rows = stmt
            .query_map([], |row| {
                let status_raw: String = row.get(3)?;
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    status_raw,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .context("querying tasks")?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title, description, status, assignee, branch, pr_url, review_notes) =
                row.context("reading task row")?;
            out.push(Task {
                id: id as u32,
                title,
                description,
                status: Status::from_db(&status)?,
                assignee,
                branch,
                pr_url,
                review_notes,
            });
        }
        Ok(out)
    }

    pub fn get(&self, id: u32) -> Option<Task> {
        self.load_tasks().ok()?.into_iter().find(|t| t.id == id)
    }

    pub fn update_task_fields(&self, t: &Task) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tasks SET title = ?1, description = ?2, status = ?3, assignee = ?4, branch = ?5, pr_url = ?6, review_notes = ?7 WHERE id = ?8",
                rusqlite::params![
                    t.title,
                    t.description,
                    t.status.as_str(),
                    t.assignee,
                    t.branch,
                    t.pr_url,
                    t.review_notes,
                    t.id
                ],
            )
            .context("updating task")?;
        Ok(())
    }

    pub fn update(&self, id: u32, f: impl FnOnce(&mut Task)) -> Result<()> {
        let mut t = self
            .get(id)
            .with_context(|| format!("task #{id} not found"))?;
        f(&mut t);
        self.update_task_fields(&t)
    }
}
