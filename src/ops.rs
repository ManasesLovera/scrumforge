use anyhow::{bail, Context, Result};

use crate::agents;
use crate::board::{Board, Status};

pub fn add_backlog_task(board: &mut Board, rest: &str) -> Result<String> {
    let (title, desc) = match rest.split_once('|') {
        Some((t, d)) => (t.trim(), d.trim()),
        None => (rest, ""),
    };
    let t = board.add_task(title, desc)?;
    Ok(format!("added task #{}: {}", t.id, t.title))
}

pub fn request(board: &mut Board, text: &str) -> Result<Vec<String>> {
    if text.is_empty() {
        bail!("usage: request <text>");
    }
    let mut lines = Vec::new();
    for (title, desc, assignee) in agents::scrum_master_plan(board, text)? {
        let t = board.add_task(&title, &desc)?;
        board.update(t.id, |task| {
            task.status = Status::Assigned;
            task.assignee = Some(assignee.clone());
        })?;
        lines.push(format!("task #{} -> {assignee}: {title}", t.id));
    }
    if lines.is_empty() {
        bail!("scrum master produced no tasks");
    }
    Ok(lines)
}

pub fn run_task(board: &mut Board, id: u32) -> Result<String> {
    let assignee = board
        .get(id)
        .context("task not found")?
        .assignee
        .unwrap_or_else(|| "developer".into());
    match assignee.as_str() {
        "reviewer" => {
            agents::reviewer_work(board, id)?;
            let done = board.get(id).unwrap().status == Status::Done;
            Ok(if done {
                format!("task #{id}: PR approved and merged")
            } else {
                format!("task #{id}: changes requested — run rework when ready")
            })
        }
        _ => {
            agents::developer_work(board, id)?;
            match board.get(id).and_then(|t| t.pr_url) {
                Some(url) => Ok(format!("task #{id}: PR opened: {url}")),
                None => Ok(format!("task #{id}: developer finished (no PR url captured)")),
            }
        }
    }
}

pub fn assign(board: &mut Board, id: u32, who: &str) -> Result<String> {
    let who = who.trim();
    if who.is_empty() {
        bail!("usage: assign <id> <developer|reviewer|name>");
    }
    board.update(id, |task| {
        task.assignee = Some(who.to_string());
        if task.status == Status::Backlog {
            task.status = Status::Assigned;
        }
    })?;
    Ok(format!("task #{id} assigned to {who}"))
}

pub fn review(board: &mut Board, id: u32, feedback: &str) -> Result<String> {
    let feedback = feedback.trim();
    if feedback.is_empty() {
        bail!("usage: review <id> <feedback>");
    }
    board.update(id, |task| {
        task.status = Status::InProgress;
        task.review_notes = Some(feedback.to_string());
    })?;
    Ok(format!(
        "task #{id}: review sent back in progress — {feedback}"
    ))
}

pub fn rework(board: &mut Board, id: u32) -> Result<String> {
    agents::developer_rework(board, id)?;
    Ok(format!("task #{id}: rework pushed, back in review"))
}
