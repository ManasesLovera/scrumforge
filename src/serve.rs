use anyhow::Result;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::board::{Board, Status, Task};
use crate::ops;

const INDEX_HTML: &str = include_str!("web/index.html");

#[derive(Clone)]
struct App {
    board: Arc<Mutex<Board>>,
    running: Arc<Mutex<HashSet<u32>>>,
    last_error: Arc<Mutex<Option<String>>>,
    repo: Arc<PathBuf>,
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

#[derive(Deserialize)]
struct MoveReq {
    status: String,
}

#[derive(Deserialize)]
struct NewTask {
    title: String,
    #[serde(default)]
    description: String,
}

pub fn parse_port(args: &[String]) -> Result<u16> {
    let mut port = 3030;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg.strip_prefix("--port=") {
            port = value.parse()?;
        } else if arg == "--port" || arg == "-p" {
            let value = args
                .get(i + 1)
                .ok_or_else(|| anyhow::anyhow!("missing value for {arg}"))?;
            port = value.parse()?;
            i += 1;
        }
        i += 1;
    }
    Ok(port)
}

pub fn run(board: Board, port: u16) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(serve(board, port))
}

async fn serve(board: Board, port: u16) -> Result<()> {
    let repo = Arc::new(board.repo_path.clone());
    let app = App {
        board: Arc::new(Mutex::new(board)),
        running: Arc::new(Mutex::new(HashSet::new())),
        last_error: Arc::new(Mutex::new(None)),
        repo: repo.clone(),
    };

    let router = Router::new()
        .route("/", get(index))
        .route("/api/tasks", get(list_tasks).post(create_task))
        .route("/api/tasks/{id}/status", put(move_task))
        .route("/api/tasks/{id}/run", post(run_task))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("scrumforge web view: http://127.0.0.1:{port}");
    println!("project: {}", repo.display());
    axum::serve(listener, router).await?;
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn task_json(task: &Task) -> Value {
    json!({
        "id": task.id,
        "title": task.title,
        "description": task.description,
        "status": task.status.as_str(),
        "assignee": task.assignee,
        "branch": task.branch,
        "pr_url": task.pr_url,
        "review_notes": task.review_notes,
    })
}

fn allowed(from: &Status, to: &Status) -> bool {
    use Status::*;
    matches!(
        (from, to),
        (Backlog, Assigned)
            | (Assigned, Backlog | InProgress)
            | (InProgress, Assigned | InReview)
            | (InReview, InProgress | ChangesRequested | Done)
            | (ChangesRequested, InProgress | InReview)
            | (Done, InReview)
    )
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn list_tasks(State(app): State<App>) -> Json<Value> {
    let tasks: Vec<Value> = lock(&app.board).tasks().iter().map(task_json).collect();
    let running: Vec<u32> = lock(&app.running).iter().copied().collect();
    let error = lock(&app.last_error).clone();
    Json(json!({
        "repo": app.repo.to_string_lossy(),
        "tasks": tasks,
        "running": running,
        "error": error,
    }))
}

async fn create_task(
    State(app): State<App>,
    Json(req): Json<NewTask>,
) -> Result<Json<Value>, ApiError> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "title is required".into()));
    }
    let mut board = lock(&app.board);
    let task = board
        .add_task(title, req.description.trim())
        .map_err(internal)?;
    Ok(Json(task_json(&task)))
}

async fn move_task(
    State(app): State<App>,
    Path(id): Path<u32>,
    Json(req): Json<MoveReq>,
) -> Result<Json<Value>, ApiError> {
    let to = Status::from_db(&req.status)
        .map_err(|_| ApiError(StatusCode::BAD_REQUEST, format!("unknown status: {}", req.status)))?;
    let board = lock(&app.board);
    let task = board
        .get(id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("task #{id} not found")))?;
    if task.status != to && !allowed(&task.status, &to) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("cannot move #{id} from {} to {}", task.status, to),
        ));
    }
    board
        .update(id, |t| t.status = to.clone())
        .map_err(internal)?;
    let updated = board
        .get(id)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("task #{id} not found")))?;
    Ok(Json(task_json(&updated)))
}

async fn run_task(
    State(app): State<App>,
    Path(id): Path<u32>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    if lock(&app.board).get(id).is_none() {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            format!("task #{id} not found"),
        ));
    }
    if !lock(&app.running).insert(id) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("task #{id} is already running"),
        ));
    }

    let worker = app.clone();
    std::thread::spawn(move || {
        let result = {
            let mut board = lock(&worker.board);
            ops::run_task(&mut board, id)
        };
        match result {
            Ok(message) => {
                *lock(&worker.last_error) = None;
                eprintln!("{message}");
            }
            Err(error) => {
                *lock(&worker.last_error) = Some(format!("task #{id}: {error:#}"));
                eprintln!("task #{id} failed: {error:#}");
            }
        }
        lock(&worker.running).remove(&id);
    });

    Ok((StatusCode::ACCEPTED, Json(json!({ "started": id }))))
}

fn internal(error: anyhow::Error) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
}
