use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use crate::board::{Board, Status, Task};

/// PID of the subprocess currently running, if any. Each child gets its own
/// process group, so cancelling can take down the whole tree — agents spawn
/// helpers of their own, and killing only the parent would orphan them.
static CURRENT_CHILD: Mutex<Option<i32>> = Mutex::new(None);

/// Set while a cancel is in flight, so the *next* step of a multi-command
/// operation refuses to start rather than racing past the kill.
static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Arm cancellation for a fresh operation. Call before starting one.
pub fn reset_cancel() {
    CANCELLED.store(false, Ordering::SeqCst);
}

/// Kill the running subprocess tree, if any, and make later steps of the same
/// operation bail out instead of continuing.
pub fn cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
    let pgid = CURRENT_CHILD.lock().ok().and_then(|g| *g);
    if let Some(pgid) = pgid {
        // Safety: `pgid` names a process group we created via `process_group(0)`.
        // A stale pgid is harmless here — killpg just reports ESRCH.
        unsafe { libc::killpg(pgid, libc::SIGKILL) };
    }
}

/// Run `cmd` to completion with its output captured, tracked so [`cancel`] can
/// kill it mid-flight.
///
/// stdin is `/dev/null` deliberately: the TUI holds the terminal in raw mode, and
/// a child that inherited stdin would swallow the user's keystrokes.
fn output_tracked(cmd: &mut Command) -> Result<Output> {
    if CANCELLED.load(Ordering::SeqCst) {
        bail!("cancelled");
    }
    let child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()?;
    if let Ok(mut slot) = CURRENT_CHILD.lock() {
        *slot = Some(child.id() as i32);
    }
    let waited = child.wait_with_output();
    if let Ok(mut slot) = CURRENT_CHILD.lock() {
        *slot = None;
    }
    let out = waited?;
    // A killed child looks like an ordinary failure; report the real reason.
    if CANCELLED.load(Ordering::SeqCst) {
        bail!("cancelled");
    }
    Ok(out)
}

/// Run a non-interactive opencode agent turn in `workdir` and return its text reply.
pub fn ask_agent(role: &str, workdir: &Path, prompt: &str) -> Result<String> {
    let full_prompt = format!(
        "You are the {role} on a scrum team. Reply with a single JSON object, \
         no markdown fences, no extra text. {prompt}"
    );
    let mut cmd = Command::new("opencode");
    cmd.arg("run").arg(&full_prompt).current_dir(workdir);
    let out = output_tracked(&mut cmd).context("spawning opencode")?;
    if !out.status.success() {
        bail!(
            "opencode failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Extract the first JSON object from agent output.
fn parse_json(reply: &str) -> Result<Value> {
    let start = reply
        .find('{')
        .with_context(|| format!("no JSON object in agent reply: {reply}"))?;
    let end = reply
        .rfind('}')
        .with_context(|| format!("unterminated JSON in agent reply: {reply}"))?;
    serde_json::from_str(&reply[start..=end]).context("parsing agent JSON reply")
}

fn worktree_root(repo_path: &Path, branch: &str) -> PathBuf {
    let name = repo_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".into());
    home_worktrees().join(name).join(branch)
}

fn home_worktrees() -> PathBuf {
    home_dir().join("dev").join("worktrees")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .expect("HOME not set")
}

fn run(cmd: &str, args: &[&str], dir: &Path) -> Result<String> {
    let mut command = Command::new(cmd);
    command.args(args).current_dir(dir);
    let out = output_tracked(&mut command).with_context(|| format!("running {cmd}"))?;
    if !out.status.success() {
        bail!(
            "{cmd} {} failed ({}): {}",
            args.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Scrum master: break the PO's request into concrete tasks and assign each to
/// developer or reviewer. Returns list of (title, description, assignee).
pub fn scrum_master_plan(board: &Board, request: &str) -> Result<Vec<(String, String, String)>> {
    let prompt = format!(
        "The product owner says: \"{request}\"\
         \nExisting open tasks: {}\
         \nBreak this into 1-4 concrete engineering tasks. Avoid duplicating open tasks. \
         Respond as JSON: {{\"tasks\":[{{\"title\":\"...\",\"description\":\"...\",\"assignee\":\"developer\"}}]}} \
         where assignee is \"developer\" for implementation tasks (most tasks) or \
         \"reviewer\" for review-only/audit tasks.",
        board
            .tasks()
            .iter()
            .filter(|t| t.status != Status::Done)
            .map(|t| format!("#{} {} [{}]", t.id, t.title, t.status))
            .collect::<Vec<_>>()
            .join("; "),
    );
    let reply = ask_agent("SCRUM MASTER", &board.repo_path, &prompt)?;
    let v = parse_json(&reply)?;
    let mut out = Vec::new();
    for t in v
        .get("tasks")
        .and_then(|t| t.as_array())
        .context("scrum master reply missing tasks array")?
    {
        let title = t.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let desc = t
            .get("description")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let assignee = match t.get("assignee").and_then(|x| x.as_str()).unwrap_or("developer") {
            "reviewer" => "reviewer",
            _ => "developer",
        }
        .to_string();
        if !title.is_empty() {
            out.push((title, desc, assignee));
        }
    }
    Ok(out)
}

/// Developer agent: create a worktree, implement, commit, push, open PR.
pub fn developer_work(board: &Board, id: u32) -> Result<()> {
    let task = board.get(id).context("task not found")?;
    if task.status == Status::Done {
        bail!("task #{id} is already done");
    }
    let branch = task.branch.clone().unwrap_or_else(|| format!("scrumforge/task-{id}"));
    let wt = worktree_root(&board.repo_path, &branch);

    if !wt.exists() {
        run(
            "git",
            &["worktree", "add", "-b", &branch, wt.to_string_lossy().as_ref(), "main"],
            &board.repo_path,
        )?;
    }
    board.update(id, |t| {
        t.branch = Some(branch.clone());
        t.status = Status::InProgress;
        t.assignee = Some("developer".into());
    })?;

    let prompt = format!(
        "Implement this task in the current git worktree: \
         \nTitle: {}\
         \nDescription: {}\
         \nSteps: 1) implement the change 2) run any available tests/linters and fix issues \
         3) stage and commit ALL your changes with a Conventional Commit message. \
         Do NOT push or create a PR. \
         Respond as JSON: {{\"summary\":\"...\",\"commit\":\"<commit-sha>\",\"files_changed\":n}}",
        task.title, task.description,
    );
    let reply = ask_agent("DEVELOPER", &wt, &prompt)?;
    let v = parse_json(&reply)?;

    run("git", &["push", "-u", "origin", &branch], &wt)?;
    let title = format!("feat(task-{id}): {}", task.title);
    let body = format!(
        "Closes scrum task #{}: {}\n\n{}\n\nDeveloper summary: {}",
        id,
        task.title,
        task.description,
        v.get("summary").and_then(|x| x.as_str()).unwrap_or("")
    );
    let pr_url = run(
        "gh",
        &["pr", "create", "--title", &title, "--body", &body, "--base", "main"],
        &wt,
    )?;

    board.update(id, |t| {
        t.status = Status::InReview;
        t.pr_url = Some(pr_url);
    })?;
    Ok(())
}

/// Reviewer agent: inspect the PR, approve+merge or send back.
pub fn reviewer_work(board: &Board, id: u32) -> Result<()> {
    let task = board.get(id).context("task not found")?;
    let Some(pr_url) = task.pr_url.clone() else {
        bail!("task #{id} has no PR yet; run the developer first");
    };
    let branch = task.branch.context("task missing branch")?;
    let wt = worktree_root(&board.repo_path, &branch);

    let diff = run("gh", &["pr", "diff", &pr_url], &board.repo_path)?;
    let prompt = format!(
        "Review this pull request for scrum task #{}: {}\
         \nPR: {}\
         \nDiff:\n```diff\n{diff}\n```\
         \nCheck: correctness, security, tests, style. \
         Respond as JSON: {{\"verdict\":\"approve\"|\"changes\",\"notes\":\"...\"}}",
        id, task.title, pr_url,
    );
    let reply = ask_agent("CODE REVIEWER", &board.repo_path, &prompt)?;
    let v = parse_json(&reply)?;
    let verdict = v.get("verdict").and_then(|x| x.as_str()).unwrap_or("changes");
    let notes = v.get("notes").and_then(|x| x.as_str()).unwrap_or("").to_string();

    if verdict == "approve" {
        run(
            "gh",
            &["pr", "review", &pr_url, "--approve", "--body", &format!("Scrum review approved: {notes}")],
            &board.repo_path,
        )?;
        run("gh", &["pr", "merge", &pr_url, "--squash", "--delete-branch"], &board.repo_path)?;
        run("git", &["worktree", "remove", wt.to_string_lossy().as_ref()], &board.repo_path)?;
        board.update(id, |t| {
            t.status = Status::Done;
            t.review_notes = Some(notes);
        })?;
    } else {
        board.update(id, |t| {
            t.status = Status::ChangesRequested;
            t.review_notes = Some(notes);
        })?;
    }
    Ok(())
}

/// After changes requested: developer addresses review notes on the same branch.
pub fn developer_rework(board: &Board, id: u32) -> Result<()> {
    let task = board.get(id).context("task not found")?;
    let branch = task.branch.context("task missing branch")?;
    let wt = worktree_root(&board.repo_path, &branch);
    let notes = task.review_notes.clone().unwrap_or_default();

    run("git", &["pull", "--rebase", "origin", &branch], &wt).ok();
    let prompt = format!(
        "Code review requested changes on your branch for task #{}: {}\
         \nReviewer notes: {notes}\
         \nFix the issues, run tests/linters, commit with a Conventional Commit message. \
         Do NOT push. Respond as JSON: {{\"summary\":\"...\",\"commit\":\"<sha>\"}}",
        id, task.title,
    );
    let reply = ask_agent("DEVELOPER", &wt, &prompt)?;
    parse_json(&reply)?;
    run("git", &["push"], &wt)?;
    board.update(id, |t| t.status = Status::InReview)?;
    Ok(())
}

pub fn print_task(task: &Task) {
    println!(
        "#{:<3} [{}] {} — {}{}",
        task.id,
        task.status,
        task.title,
        task.assignee.as_deref().unwrap_or("unassigned"),
        task.pr_url
            .as_deref()
            .map(|u| format!("\n      PR: {u}"))
            .unwrap_or_default()
    );
    if let Some(notes) = task.review_notes.as_deref().filter(|n| !n.is_empty()) {
        println!("      review: {notes}");
    }
}
