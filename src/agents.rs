use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
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

/// The error a cancelled step fails with. A distinct type rather than a
/// sentinel string: matching on the text would both miss a cancellation that
/// got reworded and swallow any unrelated error that happened to read the same.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// Whether `err` came from a cancelled step rather than a genuine failure.
pub fn was_cancelled(err: &anyhow::Error) -> bool {
    err.chain().any(|c| c.is::<Cancelled>())
}

/// Serializes the tests that touch [`CANCELLED`] and [`CURRENT_CHILD`]. Both
/// are process-wide, and the TUI tests reach them through `cancel_job`, so the
/// lock has to be shared across modules rather than private to one.
#[cfg(test)]
pub fn test_guard() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// Arm cancellation for a fresh operation. Call before starting one.
pub fn reset_cancel() {
    CANCELLED.store(false, Ordering::SeqCst);
}

/// Kill the running subprocess tree, if any, and make later steps of the same
/// operation bail out instead of continuing.
pub fn cancel() {
    CANCELLED.store(true, Ordering::SeqCst);
    // Taking the lock blocks until any in-flight spawn has registered its pid,
    // so a child started a moment ago cannot escape the kill.
    let pgid = *lock_child();
    if let Some(pgid) = pgid {
        kill_group(pgid);
    }
}

/// The child registry, ignoring poisoning: a panicking worker must not make the
/// remaining ones unkillable.
fn lock_child() -> std::sync::MutexGuard<'static, Option<i32>> {
    CURRENT_CHILD.lock().unwrap_or_else(|e| e.into_inner())
}

fn kill_group(pgid: i32) {
    // Safety: `pgid` names a process group we created via `process_group(0)`.
    // A stale pgid is harmless here — killpg just reports ESRCH.
    unsafe { libc::killpg(pgid, libc::SIGKILL) };
}

/// Run `cmd` to completion with its output captured, tracked so [`cancel`] can
/// kill it mid-flight.
///
/// stdin is `/dev/null` deliberately: the TUI holds the terminal in raw mode, and
/// a child that inherited stdin would swallow the user's keystrokes.
fn output_tracked(cmd: &mut Command) -> Result<Output> {
    if CANCELLED.load(Ordering::SeqCst) {
        bail!(Cancelled);
    }
    // Spawn and register under one lock. If they were separate steps, a cancel
    // landing in between would find an empty slot, kill nothing, and leave us
    // blocked in `wait_with_output` until the agent finished on its own.
    let mut child = {
        let mut slot = lock_child();
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()?;
        let pgid = child.id() as i32;
        *slot = Some(pgid);
        // The check at the top of this function raced too: cancel may have set
        // the flag and taken the lock before us, finding nothing to kill.
        if CANCELLED.load(Ordering::SeqCst) {
            kill_group(pgid);
        }
        child
    };
    let out = collect_output(&mut child)?;
    // A killed child looks like an ordinary failure; report the real reason.
    if CANCELLED.load(Ordering::SeqCst) {
        bail!(Cancelled);
    }
    Ok(out)
}

/// Read the child's output to EOF, deregister it, and only then reap it.
///
/// The order is what makes [`cancel`] safe. `wait_with_output` reaps while the
/// pid is still registered, so a cancel landing between the reap and the
/// deregistration would `killpg` a pid the kernel is already free to have handed
/// to somebody else. Here the pipes hit EOF first — the child and anything
/// holding them open have finished — and an unreaped pid cannot be recycled, so
/// a kill in this window is inert rather than dangerous.
fn collect_output(child: &mut Child) -> Result<Output> {
    let mut out_pipe = child.stdout.take().context("stdout was not piped")?;
    let mut err_pipe = child.stderr.take().context("stderr was not piped")?;
    // Both pipes have to be drained concurrently: a child that fills one while
    // we block on the other deadlocks.
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        out_pipe.read_to_end(&mut buf).map(|_| buf)
    });
    let mut stderr = Vec::new();
    let err_res = err_pipe.read_to_end(&mut stderr);
    let out_res = reader.join().unwrap_or_else(|_| Ok(Vec::new()));

    *lock_child() = None;

    let stdout = out_res?;
    err_res?;
    let status = child.wait()?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The hand-rolled capture replaced `wait_with_output`; it must still return
    /// both streams and the real exit status.
    #[test]
    fn output_tracked_captures_both_streams_and_the_status() {
        let _serial = test_guard();
        reset_cancel();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo out; echo err >&2; exit 3"]);
        let out = output_tracked(&mut cmd).unwrap();

        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "out");
        assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "err");
        assert_eq!(out.status.code(), Some(3));
        assert_eq!(*lock_child(), None, "child left registered after it finished");
    }

    /// More output than a pipe buffer holds: reading the two streams one after
    /// the other would deadlock here.
    #[test]
    fn output_tracked_survives_output_larger_than_a_pipe_buffer() {
        let _serial = test_guard();
        reset_cancel();
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "head -c 200000 /dev/zero; head -c 200000 /dev/zero >&2"]);
        let out = output_tracked(&mut cmd).unwrap();

        assert_eq!(out.stdout.len(), 200_000);
        assert_eq!(out.stderr.len(), 200_000);
    }

    /// A cancel already in flight must stop the next step from starting, and the
    /// failure has to be recognisable as a cancellation rather than an error.
    #[test]
    fn a_cancelled_run_refuses_to_start_and_says_why() {
        let _serial = test_guard();
        cancel();
        let mut cmd = Command::new("true");
        let err = output_tracked(&mut cmd).unwrap_err();
        reset_cancel();

        assert!(was_cancelled(&err), "not recognised as a cancellation: {err:#}");
    }

    /// The whole point of the type: an unrelated failure whose text happens to
    /// read "cancelled" must not be mistaken for one.
    #[test]
    fn was_cancelled_ignores_errors_that_merely_say_cancelled() {
        let err = anyhow::anyhow!("the reviewer cancelled the merge");
        assert!(!was_cancelled(&err));
        assert!(!was_cancelled(&anyhow::anyhow!("cancelled")));
    }

    /// The whole point of the machinery: a cancel must take down a child that is
    /// still running, rather than leaving the caller blocked until it finishes.
    #[test]
    fn cancel_kills_a_child_that_is_still_running() {
        let _serial = test_guard();
        reset_cancel();
        let worker = std::thread::spawn(|| {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "sleep 30"]);
            output_tracked(&mut cmd)
        });

        let start = std::time::Instant::now();
        while lock_child().is_none() {
            assert!(start.elapsed() < Duration::from_secs(5), "child never registered");
            std::thread::sleep(Duration::from_millis(5));
        }
        cancel();
        let err = worker.join().expect("worker panicked").unwrap_err();
        reset_cancel();

        assert!(was_cancelled(&err), "not reported as cancelled: {err:#}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "waited out the child instead of killing it: {:?}",
            start.elapsed()
        );
        assert_eq!(*lock_child(), None, "child left registered after the kill");
    }
}
