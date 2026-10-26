use anyhow::{Context, Result};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::board::{Project, Status, Store};
use crate::ops;
use crate::resolve::{self, validate_route};

const INDEX_HTML: &str = include_str!("web/index.html");

#[derive(Clone)]
struct App {
    store: Arc<Mutex<Store>>,
    running: Arc<Mutex<HashSet<u32>>>,
    last_error: Arc<Mutex<Option<String>>>,
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

#[derive(Deserialize)]
struct MoveReq {
    status: Option<String>,
    sprint: Option<String>,
}

#[derive(Deserialize)]
struct NewTask {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    acceptance_criteria: String,
    #[serde(default)]
    sprint: Option<u32>,
    #[serde(default)]
    spec_slug: Option<String>,
    #[serde(default)]
    change_slug: Option<String>,
}

#[derive(Deserialize)]
struct NewProject {
    route: String,
    path: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    methodology: Option<String>,
    #[serde(default)]
    default_branch: Option<String>,
    #[serde(default)]
    worktree_root: Option<String>,
    #[serde(default)]
    max_parallel_runs: Option<u32>,
}

#[derive(Deserialize)]
struct NewSprint {
    name: String,
    #[serde(default)]
    goal: String,
    #[serde(default)]
    starts_at: Option<String>,
    #[serde(default)]
    ends_at: Option<String>,
    #[serde(default)]
    capacity: Option<f64>,
}

#[derive(Deserialize)]
struct SprintAction {
    action: String,
    #[serde(default)]
    move_unfinished: Option<String>,
}

#[derive(Deserialize)]
struct NewSpec {
    slug: String,
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    body_md: String,
}

#[derive(Deserialize)]
struct TaskQuery {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    sprint: Option<String>,
}

pub fn parse_port(args: &[String]) -> Result<u16> {
    let mut port = 3030;
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(value) = arg.strip_prefix("--port=") {
            port = value.parse()?;
        } else if arg == "--port" || arg == "-p" {
            let value = args.get(i + 1).context("missing value for --port")?;
            port = value.parse()?;
            i += 1;
        }
        i += 1;
    }
    Ok(port)
}

pub fn run(store: Store, port: u16) -> Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(serve(store, port))
}

async fn serve(store: Store, port: u16) -> Result<()> {
    let db_path = store.db_path.clone();
    let app = App {
        store: Arc::new(Mutex::new(store)),
        running: Arc::new(Mutex::new(HashSet::new())),
        last_error: Arc::new(Mutex::new(None)),
    };

    let router = Router::new()
        .route("/", get(index))
        .route("/p/{route}", get(index))
        .route("/p/{route}/{*rest}", get(index))
        .route("/api/projects", get(list_projects).post(create_project))
        .route("/api/projects/{route}", get(get_project))
        .route(
            "/api/projects/{route}/tasks",
            get(list_tasks).post(create_task),
        )
        .route("/api/projects/{route}/tasks/{id}/status", put(move_task))
        .route("/api/projects/{route}/tasks/{id}/run", post(run_task))
        .route(
            "/api/projects/{route}/sprints",
            get(list_sprints).post(create_sprint),
        )
        .route(
            "/api/projects/{route}/sprints/{number}",
            patch(update_sprint),
        )
        .route(
            "/api/projects/{route}/specs",
            get(list_specs).post(create_spec),
        )
        .route("/api/projects/{route}/specs/{slug}", get(get_spec))
        .route(
            "/api/projects/{route}/specs/{slug}/approve",
            post(approve_spec),
        )
        .route("/api/projects/{route}/specs/{slug}/plan", post(plan_spec))
        .route("/api/projects/{route}/changes", get(list_changes))
        .route("/api/projects/{route}/events", get(list_events))
        .with_state(app);

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("scrumforge web view: http://127.0.0.1:{port}");
    println!("database: {}", db_path.display());
    axum::serve(listener, router).await?;
    Ok(())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn internal(error: anyhow::Error) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{error:#}"))
}

fn bad(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, message.into())
}

fn not_found(message: impl Into<String>) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, message.into())
}

fn bad_err(error: anyhow::Error) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, format!("{error:#}"))
}

fn project_for(store: &Store, route: &str) -> Result<Project, ApiError> {
    store
        .project_by_route(route)
        .map_err(internal)?
        .ok_or_else(|| not_found(format!("project '{route}' not found")))
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

fn project_json(store: &Store, project: &Project) -> Result<Value> {
    let active = store.active_sprint(project.id)?;
    let (backlog, done, specs, sprints) = store.counts(project.id)?;
    Ok(json!({
        "id": project.id,
        "route": project.route,
        "name": project.name,
        "repo_path": project.repo_path.to_string_lossy(),
        "default_branch": project.default_branch,
        "worktree_root": project.worktree_root.to_string_lossy(),
        "methodology": project.methodology.as_str(),
        "max_parallel_runs": project.max_parallel_runs,
        "active_sprint": active.map(|s| json!({
            "number": s.number, "name": s.name, "goal": s.goal, "status": s.status.as_str()
        })),
        "counts": {
            "backlog": backlog,
            "done": done,
            "specs": specs,
            "sprints": sprints,
        },
    }))
}

async fn list_projects(State(app): State<App>) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let projects = store.projects().map_err(internal)?;
    let values = projects
        .iter()
        .map(|p| project_json(&store, p))
        .collect::<Result<Vec<_>>>()
        .map_err(internal)?;
    Ok(Json(json!({ "projects": values })))
}

async fn create_project(
    State(app): State<App>,
    Json(req): Json<NewProject>,
) -> Result<Json<Value>, ApiError> {
    validate_route(&req.route).map_err(|e| bad(e.to_string()))?;
    let repo_path = resolve::validate_project_path(&resolve::expand_path(&req.path))
        .map_err(|e| bad(e.to_string()))?;
    let methodology = resolve::parse_methodology(req.methodology.as_deref().unwrap_or("scrum"))
        .map_err(|e| bad(e.to_string()))?;
    let worktree_root = req
        .worktree_root
        .as_deref()
        .map(resolve::expand_path)
        .unwrap_or_else(|| resolve::home_dir().join("dev").join("worktrees"));
    let store = lock(&app.store);
    if store
        .project_by_route(&req.route)
        .map_err(internal)?
        .is_some()
    {
        return Err(ApiError(
            StatusCode::CONFLICT,
            format!("project route '{}' already exists", req.route),
        ));
    }
    let project = store
        .add_project(
            &req.route,
            req.name.as_deref().unwrap_or(&req.route),
            &repo_path,
            req.default_branch.as_deref().unwrap_or("main"),
            &worktree_root,
            &methodology,
            req.max_parallel_runs.unwrap_or(2),
        )
        .map_err(internal)?;
    Ok(Json(project_json(&store, &project).map_err(internal)?))
}

async fn get_project(
    State(app): State<App>,
    Path(route): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    Ok(Json(project_json(&store, &project).map_err(internal)?))
}

async fn list_tasks(
    State(app): State<App>,
    Path(route): Path<String>,
    Query(query): Query<TaskQuery>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let scope = query
        .scope
        .or(query.sprint)
        .unwrap_or_else(|| "board".into());
    let tasks = match scope.as_str() {
        "backlog" => store.backlog_tasks(project.id),
        "all" => store.tasks(project.id),
        "board" | "current" => match store.active_sprint(project.id).map_err(internal)? {
            Some(sprint) => store.sprint_tasks(project.id, sprint.id),
            None => Ok(Vec::new()),
        },
        other => {
            let number = other
                .parse::<u32>()
                .map_err(|_| bad(format!("invalid sprint scope: {other}")))?;
            let sprint = store
                .sprint_by_number(project.id, number)
                .map_err(internal)?
                .ok_or_else(|| not_found(format!("sprint {number} not found")))?;
            store.sprint_tasks(project.id, sprint.id)
        }
    }
    .map_err(internal)?;
    let values = tasks
        .iter()
        .map(|t| ops::task_json(&store, t))
        .collect::<Result<Vec<_>>>()
        .map_err(internal)?;
    let running: Vec<u32> = lock(&app.running).iter().copied().collect();
    Ok(Json(json!({
        "project": project.route,
        "scope": scope,
        "tasks": values,
        "running": running,
        "error": lock(&app.last_error).clone(),
    })))
}

async fn create_task(
    State(app): State<App>,
    Path(route): Path<String>,
    Json(req): Json<NewTask>,
) -> Result<Json<Value>, ApiError> {
    let title = req.title.trim();
    if title.is_empty() {
        return Err(bad("title is required"));
    }
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let sprint_id = match req.sprint {
        Some(number) => Some(
            store
                .sprint_by_number(project.id, number)
                .map_err(internal)?
                .ok_or_else(|| not_found(format!("sprint {number} not found")))?
                .id,
        ),
        None => None,
    };
    let status = if sprint_id.is_some() {
        Status::Todo
    } else {
        Status::Backlog
    };
    let task = store
        .add_task(
            project.id,
            title,
            req.description.trim(),
            req.acceptance_criteria.trim(),
            &status,
            None,
            sprint_id,
            req.spec_slug.as_deref().filter(|s| !s.is_empty()),
            req.change_slug.as_deref().filter(|s| !s.is_empty()),
            None,
            None,
        )
        .map_err(internal)?;
    store
        .log_event(
            project.id,
            sprint_id,
            Some(task.id),
            "web",
            "created",
            None,
            Some(status.as_str()),
            None,
        )
        .map_err(internal)?;
    Ok(Json(ops::task_json(&store, &task).map_err(internal)?))
}

async fn move_task(
    State(app): State<App>,
    Path((route, id)): Path<(String, u32)>,
    Json(req): Json<MoveReq>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    if let Some(status) = req.status.as_deref() {
        let target =
            Status::from_db(status).map_err(|_| bad(format!("unknown status: {status}")))?;
        ops::move_task(&store, &project, id, &target, "web").map_err(bad_err)?;
    }
    if let Some(sprint) = req.sprint.as_deref() {
        if sprint == "backlog" {
            ops::move_task(&store, &project, id, &Status::Backlog, "web").map_err(bad_err)?;
        } else {
            let number = sprint
                .parse::<u32>()
                .map_err(|_| bad(format!("invalid sprint: {sprint}")))?;
            ops::add_to_sprint(&store, &project, id, number, "web").map_err(bad_err)?;
        }
    }
    let task = store
        .task(project.id, id)
        .map_err(internal)?
        .ok_or_else(|| not_found(format!("task #{id} not found")))?;
    Ok(Json(ops::task_json(&store, &task).map_err(internal)?))
}

async fn run_task(
    State(app): State<App>,
    Path((route, id)): Path<(String, u32)>,
) -> Result<(StatusCode, Json<Value>), ApiError> {
    {
        let store = lock(&app.store);
        let project = project_for(&store, &route)?;
        if store.task(project.id, id).map_err(internal)?.is_none() {
            return Err(not_found(format!("task #{id} not found")));
        }
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
            let store = lock(&worker.store);
            match project_for(&store, &route) {
                Ok(project) => ops::run_task(&store, &project, id, "agent"),
                Err(error) => Err(anyhow::anyhow!(error.1)),
            }
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

async fn list_sprints(
    State(app): State<App>,
    Path(route): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let sprints = store.sprints(project.id).map_err(internal)?;
    let values = sprints
        .iter()
        .map(|s| {
            json!({
                "number": s.number,
                "name": s.name,
                "goal": s.goal,
                "status": s.status.as_str(),
                "starts_at": s.starts_at,
                "ends_at": s.ends_at,
                "capacity": s.capacity,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "sprints": values })))
}

async fn create_sprint(
    State(app): State<App>,
    Path(route): Path<String>,
    Json(req): Json<NewSprint>,
) -> Result<Json<Value>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(bad("sprint name is required"));
    }
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let sprint = store
        .add_sprint(
            project.id,
            req.name.trim(),
            req.goal.trim(),
            req.starts_at.as_deref(),
            req.ends_at.as_deref(),
            req.capacity,
        )
        .map_err(internal)?;
    store
        .log_event(
            project.id,
            Some(sprint.id),
            None,
            "web",
            "sprint-created",
            None,
            Some("planned"),
            Some(&sprint.name),
        )
        .map_err(internal)?;
    Ok(Json(json!({
        "number": sprint.number, "name": sprint.name, "status": sprint.status.as_str()
    })))
}

async fn update_sprint(
    State(app): State<App>,
    Path((route, number)): Path<(String, u32)>,
    Json(req): Json<SprintAction>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    match req.action.as_str() {
        "activate" => {
            let sprint = store.activate_sprint(project.id, number).map_err(bad_err)?;
            store
                .log_event(
                    project.id,
                    Some(sprint.id),
                    None,
                    "web",
                    "sprint-activated",
                    Some("planned"),
                    Some("active"),
                    None,
                )
                .map_err(internal)?;
            Ok(Json(
                json!({"number": sprint.number, "status": sprint.status.as_str()}),
            ))
        }
        "close" => {
            let sprint = store
                .close_sprint(project.id, number, req.move_unfinished.as_deref())
                .map_err(bad_err)?;
            store
                .log_event(
                    project.id,
                    Some(sprint.id),
                    None,
                    "web",
                    "sprint-closed",
                    Some("active"),
                    Some("completed"),
                    req.move_unfinished.as_deref(),
                )
                .map_err(internal)?;
            Ok(Json(
                json!({"number": sprint.number, "status": sprint.status.as_str()}),
            ))
        }
        other => Err(bad(format!("unknown sprint action: {other}"))),
    }
}

async fn list_specs(
    State(app): State<App>,
    Path(route): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let specs = store.specs(project.id).map_err(internal)?;
    let values = specs
        .iter()
        .map(|s| {
            json!({
                "slug": s.slug,
                "title": s.title,
                "status": s.status,
                "summary": s.summary,
                "requirements": s.requirements.len(),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "specs": values })))
}

async fn get_spec(
    State(app): State<App>,
    Path((route, slug)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let spec = store
        .spec(project.id, &slug)
        .map_err(internal)?
        .ok_or_else(|| not_found(format!("spec '{slug}' not found")))?;
    Ok(Json(json!({
        "slug": spec.slug,
        "title": spec.title,
        "status": spec.status,
        "summary": spec.summary,
        "body_md": spec.body_md,
        "plan_md": spec.plan_md,
        "requirements": spec.requirements.iter().map(|r| json!({
            "key": r.key, "kind": r.kind, "text": r.text
        })).collect::<Vec<_>>(),
    })))
}

async fn create_spec(
    State(app): State<App>,
    Path(route): Path<String>,
    Json(req): Json<NewSpec>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let mut spec = ops::create_spec(&store, &project, &req.slug, &req.title, &req.summary, "web")
        .map_err(bad_err)?;
    if !req.body_md.trim().is_empty() {
        spec.body_md = req.body_md;
        store.save_spec(&spec).map_err(internal)?;
    }
    Ok(Json(json!({"slug": spec.slug, "status": spec.status})))
}

async fn approve_spec(
    State(app): State<App>,
    Path((route, slug)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let message = ops::approve_spec(&store, &project, &slug, "web").map_err(bad_err)?;
    Ok(Json(json!({ "message": message })))
}

async fn plan_spec(
    State(app): State<App>,
    Path((route, slug)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let lines = ops::plan_spec(&store, &project, &slug, "web").map_err(bad_err)?;
    Ok(Json(json!({ "tasks": lines })))
}

async fn list_changes(
    State(app): State<App>,
    Path(route): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let changes = store.changes(project.id).map_err(internal)?;
    let values = changes
        .iter()
        .map(|c| {
            json!({
                "slug": c.slug,
                "spec_slug": c.spec_slug,
                "title": c.title,
                "status": c.status,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "changes": values })))
}

async fn list_events(
    State(app): State<App>,
    Path(route): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let store = lock(&app.store);
    let project = project_for(&store, &route)?;
    let events = store.events(project.id, 100).map_err(internal)?;
    let values = events
        .iter()
        .map(|e| {
            json!({
                "id": e.id,
                "actor": e.actor,
                "verb": e.verb,
                "task_id": e.task_id,
                "sprint_id": e.sprint_id,
                "from_status": e.from_status,
                "to_status": e.to_status,
                "detail": e.detail,
                "ts": e.ts,
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(json!({ "events": values })))
}
