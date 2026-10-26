use anyhow::{Context, Result, bail};
use std::fmt::Write as _;

use crate::agents;
use crate::board::{Methodology, Project, Spec, Status, Store, Task};

#[allow(clippy::too_many_arguments)]
fn event(
    store: &Store,
    project: &Project,
    task_id: Option<u32>,
    actor: &str,
    verb: &str,
    from: Option<&str>,
    to: Option<&str>,
    detail: Option<&str>,
) -> Result<()> {
    store.log_event(project.id, None, task_id, actor, verb, from, to, detail)
}

pub fn add_backlog_task(
    store: &Store,
    project: &Project,
    rest: &str,
    sprint: Option<u32>,
    actor: &str,
) -> Result<String> {
    let (title, desc) = match rest.split_once('|') {
        Some((t, d)) => (t.trim(), d.trim()),
        None => (rest.trim(), ""),
    };
    if title.is_empty() {
        bail!("usage: backlog <title> [| <description>]");
    }
    let task = store.add_task(
        project.id,
        title,
        desc,
        "",
        &Status::Backlog,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    event(
        store,
        project,
        Some(task.id),
        actor,
        "created",
        None,
        Some("backlog"),
        None,
    )?;
    if let Some(number) = sprint {
        add_to_sprint(store, project, task.id, number, actor)?;
        return Ok(format!(
            "added task #{}: {} (sprint {number})",
            task.id, task.title
        ));
    }
    Ok(format!("added task #{}: {}", task.id, task.title))
}

pub fn request(store: &Store, project: &Project, text: &str, actor: &str) -> Result<Vec<String>> {
    if text.trim().is_empty() {
        bail!("usage: request <text>");
    }
    match project.methodology {
        Methodology::Scrum => scrum_request(store, project, text, actor),
        Methodology::Sdd => sdd_request(store, project, text, actor),
    }
}

fn scrum_request(store: &Store, project: &Project, text: &str, actor: &str) -> Result<Vec<String>> {
    let mut lines = Vec::new();
    for (title, desc, assignee) in agents::scrum_master_plan(store, project, text, actor)? {
        let task = store.add_task(
            project.id,
            &title,
            &desc,
            "",
            &Status::Backlog,
            Some(&assignee),
            None,
            None,
            None,
            None,
            None,
        )?;
        event(
            store,
            project,
            Some(task.id),
            actor,
            "created",
            None,
            Some("backlog"),
            Some("planned by scrum master"),
        )?;
        lines.push(format!("task #{} -> {assignee}: {title}", task.id));
    }
    if lines.is_empty() {
        bail!("scrum master produced no tasks");
    }
    Ok(lines)
}

fn sdd_request(store: &Store, project: &Project, text: &str, actor: &str) -> Result<Vec<String>> {
    let draft = agents::sdd_spec_draft(project, text, actor)?;
    if store.spec(project.id, &draft.slug)?.is_some() {
        bail!("spec '{}' already exists", draft.slug);
    }
    let requirements_json = serde_json::to_string(
        &draft
            .requirements
            .iter()
            .map(|r| serde_json::json!({"key": r.key, "text": r.text, "kind": r.kind}))
            .collect::<Vec<_>>(),
    )?;
    let spec = store.add_spec(
        project.id,
        &draft.slug,
        &draft.title,
        "draft",
        &draft.summary,
        &draft.body_md,
        &draft.plan_md,
        &requirements_json,
        Some(actor),
    )?;
    event(
        store,
        project,
        None,
        actor,
        "spec-created",
        None,
        Some("draft"),
        Some(&format!("spec {}", spec.slug)),
    )?;
    Ok(vec![
        format!("draft spec '{}': {}", spec.slug, spec.title),
        format!(
            "{} requirement(s); approve with: scrumforge spec approve {}",
            spec.requirements.len(),
            spec.slug
        ),
    ])
}

pub fn run_task(store: &Store, project: &Project, id: u32, actor: &str) -> Result<String> {
    let task = store
        .task(project.id, id)?
        .with_context(|| format!("task #{id} not found"))?;
    if let Some(assignee) = task.assignee.as_deref() {
        ensure_assignable(store, project, assignee)?;
    }
    if task.status == Status::Backlog {
        bail!("task #{id} is in the backlog; move it into a sprint before running it");
    }
    if task.status == Status::Done {
        bail!("task #{id} is already done");
    }
    if task.status == Status::InReview {
        agents::reviewer_work(store, project, id, actor)?;
        let updated = store.task(project.id, id)?.context("task disappeared")?;
        event(
            store,
            project,
            Some(id),
            actor,
            "reviewed",
            Some("in-review"),
            Some(updated.status.as_str()),
            updated.review_notes.as_deref(),
        )?;
        return Ok(if updated.status == Status::Done {
            format!("task #{id}: PR approved and merged")
        } else {
            format!("task #{id}: changes requested — run rework when ready")
        });
    }

    let previous = task.status.clone();
    let assignee = task.assignee.clone().unwrap_or_else(|| "developer".into());
    if store.claim_task(project.id, id, &assignee)?.is_none() {
        bail!(
            "task #{id} cannot start from status {} (expected todo or changes-requested)",
            task.status
        );
    }
    event(
        store,
        project,
        Some(id),
        actor,
        "claimed",
        Some(previous.as_str()),
        Some("in-progress"),
        Some(&assignee),
    )?;
    let result = agents::developer_work(store, project, id, actor);
    match result {
        Ok(()) => {
            let updated = store.task(project.id, id)?.context("task disappeared")?;
            event(
                store,
                project,
                Some(id),
                actor,
                "pr-opened",
                Some("in-progress"),
                Some(updated.status.as_str()),
                updated.pr_url.as_deref(),
            )?;
            Ok(match updated.pr_url {
                Some(url) => format!("task #{id}: PR opened: {url}"),
                None => format!("task #{id}: developer finished (no PR url captured)"),
            })
        }
        Err(error) => {
            store.update_task(project.id, id, |t| t.status = previous.clone())?;
            event(
                store,
                project,
                Some(id),
                actor,
                "run-failed",
                Some("in-progress"),
                Some(previous.as_str()),
                Some(&format!("{error:#}")),
            )?;
            Err(error)
        }
    }
}

fn ensure_assignable(store: &Store, project: &Project, who: &str) -> Result<()> {
    if let Some(user) = store.user_by_handle(who)?
        && !store.user_can_access_project(user.id, project.id)?
    {
        bail!(
            "user '{}' is not scoped to project '{}'; add it with: scrumforge user scope {} {}",
            user.handle,
            project.route,
            user.handle,
            project.route
        );
    }
    Ok(())
}

pub fn assign(store: &Store, project: &Project, id: u32, who: &str, actor: &str) -> Result<String> {
    let who = who.trim();
    if who.is_empty() {
        bail!("usage: assign <id> <developer|reviewer|name>");
    }
    ensure_assignable(store, project, who)?;
    let task = store.update_task(project.id, id, |task| {
        task.assignee = Some(who.to_string());
    })?;
    event(
        store,
        project,
        Some(id),
        actor,
        "assigned",
        Some(task.status.as_str()),
        Some(task.status.as_str()),
        Some(who),
    )?;
    Ok(format!("task #{id} assigned to {who}"))
}

pub fn review(
    store: &Store,
    project: &Project,
    id: u32,
    feedback: &str,
    actor: &str,
) -> Result<String> {
    let feedback = feedback.trim();
    if feedback.is_empty() {
        bail!("usage: review <id> <feedback>");
    }
    let task = store.update_task(project.id, id, |task| {
        task.status = Status::InProgress;
        task.review_notes = Some(feedback.to_string());
    })?;
    event(
        store,
        project,
        Some(id),
        actor,
        "review-feedback",
        Some("in-review"),
        Some(task.status.as_str()),
        Some(feedback),
    )?;
    Ok(format!(
        "task #{id}: review sent back in progress — {feedback}"
    ))
}

pub fn rework(store: &Store, project: &Project, id: u32, actor: &str) -> Result<String> {
    let task = store
        .task(project.id, id)?
        .with_context(|| format!("task #{id} not found"))?;
    agents::developer_rework(store, project, id, actor)?;
    event(
        store,
        project,
        Some(id),
        actor,
        "reworked",
        Some(task.status.as_str()),
        Some("in-review"),
        None,
    )?;
    Ok(format!("task #{id}: rework pushed, back in review"))
}

pub fn move_task(
    store: &Store,
    project: &Project,
    id: u32,
    target: &Status,
    actor: &str,
) -> Result<String> {
    let task = store
        .task(project.id, id)?
        .with_context(|| format!("task #{id} not found"))?;
    if !Status::allowed(&task.status, target) {
        bail!("cannot move #{id} from {} to {}", task.status, target);
    }
    if *target == Status::Todo && task.sprint_id.is_none() {
        bail!("task #{id} is not in a sprint; add it to a sprint first");
    }
    store.update_task(project.id, id, |t| {
        t.status = target.clone();
        if *target == Status::Backlog {
            t.sprint_id = None;
        }
    })?;
    event(
        store,
        project,
        Some(id),
        actor,
        "moved",
        Some(task.status.as_str()),
        Some(target.as_str()),
        None,
    )?;
    Ok(format!("task #{id} moved to {target}"))
}

pub fn add_to_sprint(
    store: &Store,
    project: &Project,
    id: u32,
    number: u32,
    actor: &str,
) -> Result<String> {
    let sprint = store
        .sprint_by_number(project.id, number)?
        .with_context(|| format!("sprint {number} not found"))?;
    if sprint.status == crate::board::SprintStatus::Completed {
        bail!("sprint {number} is completed");
    }
    let task = store.update_task(project.id, id, |t| {
        t.sprint_id = Some(sprint.id);
        if t.status == Status::Backlog {
            t.status = Status::Todo;
        }
    })?;
    event(
        store,
        project,
        Some(id),
        actor,
        "scheduled",
        Some("backlog"),
        Some(task.status.as_str()),
        Some(&format!("sprint {}", sprint.number)),
    )?;
    Ok(format!("task #{id} added to sprint {}", sprint.number))
}

pub fn create_spec(
    store: &Store,
    project: &Project,
    slug: &str,
    title: &str,
    request_text: &str,
    actor: &str,
) -> Result<Spec> {
    let slug = agents::slugify(slug);
    if slug.is_empty() {
        bail!("spec slug is required");
    }
    if store.spec(project.id, &slug)?.is_some() {
        bail!("spec '{slug}' already exists");
    }
    let summary = request_text.trim().to_string();
    let body = if summary.is_empty() {
        format!("# {title}\n")
    } else {
        format!("# {title}\n\n## Summary\n\n{summary}\n")
    };
    let spec = store.add_spec(
        project.id,
        &slug,
        title,
        "draft",
        &summary,
        &body,
        "",
        "[]",
        Some(actor),
    )?;
    event(
        store,
        project,
        None,
        actor,
        "spec-created",
        None,
        Some("draft"),
        Some(&spec.slug),
    )?;
    Ok(spec)
}

pub fn approve_spec(store: &Store, project: &Project, slug: &str, actor: &str) -> Result<String> {
    let mut spec = store
        .spec(project.id, slug)?
        .with_context(|| format!("spec '{slug}' not found"))?;
    spec.status = "approved".into();
    store.save_spec(&spec)?;
    event(
        store,
        project,
        None,
        actor,
        "spec-approved",
        Some("draft"),
        Some("approved"),
        Some(slug),
    )?;
    Ok(format!("spec '{slug}' approved"))
}

pub fn plan_spec(store: &Store, project: &Project, slug: &str, actor: &str) -> Result<Vec<String>> {
    let mut spec = store
        .spec(project.id, slug)?
        .with_context(|| format!("spec '{slug}' not found"))?;
    if spec.status != "approved" {
        bail!(
            "spec '{slug}' must be approved before planning (status: {})",
            spec.status
        );
    }
    let existing = store
        .tasks(project.id)?
        .into_iter()
        .filter(|t| t.spec_slug.as_deref() == Some(slug))
        .count();
    if existing > 0 {
        bail!("spec '{slug}' already has {existing} task(s)");
    }
    let mut lines = Vec::new();
    if spec.requirements.is_empty() {
        let task = store.add_task(
            project.id,
            &spec.title,
            &spec.summary,
            "",
            &Status::Backlog,
            Some("developer"),
            None,
            Some(slug),
            None,
            None,
            None,
        )?;
        lines.push(format!("task #{}: {}", task.id, task.title));
    } else {
        for requirement in &spec.requirements {
            let task = store.add_task(
                project.id,
                &format!("{}: {}", spec.title, requirement.text),
                &spec.summary,
                &requirement.text,
                &Status::Backlog,
                Some("developer"),
                None,
                Some(slug),
                None,
                Some(&requirement.key),
                None,
            )?;
            lines.push(format!(
                "task #{} [{}]: {}",
                task.id, requirement.key, requirement.text
            ));
        }
    }
    spec.status = "implementing".into();
    store.save_spec(&spec)?;
    event(
        store,
        project,
        None,
        actor,
        "spec-planned",
        Some("approved"),
        Some("implementing"),
        Some(slug),
    )?;
    Ok(lines)
}

pub fn export_spec(store: &Store, project: &Project, slug: &str) -> Result<String> {
    let spec = store
        .spec(project.id, slug)?
        .with_context(|| format!("spec '{slug}' not found"))?;
    let dir = project.repo_path.join("specs").join(&spec.slug);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("spec.md"), &spec.body_md)?;
    std::fs::write(dir.join("plan.md"), &spec.plan_md)?;
    let mut tasks = String::new();
    for task in store
        .tasks(project.id)?
        .into_iter()
        .filter(|t| t.spec_slug.as_deref() == Some(slug))
    {
        let _ = writeln!(tasks, "- [{}] #{} {}", task.status, task.id, task.title);
    }
    std::fs::write(dir.join("tasks.md"), tasks)?;
    Ok(format!("exported spec '{slug}' to {}", dir.display()))
}

pub fn archive_spec(store: &Store, project: &Project, slug: &str, actor: &str) -> Result<String> {
    let mut spec = store
        .spec(project.id, slug)?
        .with_context(|| format!("spec '{slug}' not found"))?;
    let previous = spec.status.clone();
    spec.status = "archived".into();
    store.save_spec(&spec)?;
    event(
        store,
        project,
        None,
        actor,
        "spec-archived",
        Some(&previous),
        Some("archived"),
        Some(slug),
    )?;
    Ok(format!("spec '{slug}' archived"))
}

pub fn create_change(
    store: &Store,
    project: &Project,
    spec_slug: &str,
    slug: &str,
    title: &str,
    body: &str,
    actor: &str,
) -> Result<String> {
    if store.spec(project.id, spec_slug)?.is_none() {
        bail!("spec '{spec_slug}' not found");
    }
    let slug = agents::slugify(slug);
    let change = store.add_change(project.id, spec_slug, &slug, title, body)?;
    event(
        store,
        project,
        None,
        actor,
        "change-created",
        None,
        Some("proposed"),
        Some(&format!("{} (spec {})", change.slug, change.spec_slug)),
    )?;
    Ok(format!(
        "created change '{}' for spec '{spec_slug}'",
        change.slug
    ))
}

pub fn approve_change(store: &Store, project: &Project, slug: &str, actor: &str) -> Result<String> {
    store.approve_change(project.id, slug)?;
    event(
        store,
        project,
        None,
        actor,
        "change-approved",
        Some("proposed"),
        Some("approved"),
        Some(slug),
    )?;
    Ok(format!("change '{slug}' approved"))
}

pub fn task_json(store: &Store, task: &Task) -> Result<serde_json::Value> {
    let sprint = match task.sprint_id {
        Some(id) => store
            .sprints(task.project_id)?
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| serde_json::json!({"number": s.number, "name": s.name, "status": s.status.as_str()})),
        None => None,
    };
    Ok(serde_json::json!({
        "id": task.id,
        "project_id": task.project_id,
        "sprint_id": task.sprint_id,
        "sprint": sprint,
        "title": task.title,
        "description": task.description,
        "acceptance_criteria": task.acceptance_criteria,
        "status": task.status.as_str(),
        "assignee": task.assignee,
        "branch": task.branch,
        "pr_url": task.pr_url,
        "review_notes": task.review_notes,
        "spec_slug": task.spec_slug,
        "change_slug": task.change_slug,
        "requirement_key": task.requirement_key,
        "estimate": task.estimate,
        "created_at": task.created_at,
        "updated_at": task.updated_at,
    }))
}
