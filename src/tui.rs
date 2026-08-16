use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use crate::agents;
use crate::board::{Board, Status};
use crate::help;
use crate::ops;

/// How long we block waiting for a key before looping. Also the spinner's frame
/// rate and the worst-case delay before a shutdown signal is noticed.
const TICK: Duration = Duration::from_millis(100);

/// How long a cancelled worker gets to unwind before we exit without it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

type StatusPred = fn(&Status) -> bool;
const COLUMNS: &[(&str, StatusPred)] = &[
    ("Backlog", |s| matches!(s, Status::Backlog)),
    ("Assigned", |s| matches!(s, Status::Assigned)),
    ("Working", |s| {
        matches!(s, Status::InProgress | Status::ChangesRequested)
    }),
    ("Review", |s| matches!(s, Status::InReview)),
    ("Done", |s| matches!(s, Status::Done)),
];

enum Mode {
    Normal,
    Command { prompt: String, input: String },
    Help,
    Task,
}

enum Action {
    Request(String),
    Run(u32),
    Rework(u32),
}

/// An agent operation running on a worker thread. Agents shell out to
/// `opencode`, `git` and `gh` and routinely take minutes, so they must never run
/// on the UI thread — a blocked loop cannot redraw, cannot read keys, and cannot
/// be interrupted.
struct Job {
    label: String,
    rx: mpsc::Receiver<Result<String>>,
    frame: usize,
    cancelling: bool,
    /// The user asked to leave without waiting for the worker to unwind.
    forced: bool,
}

struct App {
    board: Board,
    col: usize,
    list_states: Vec<ListState>,
    mode: Mode,
    message: String,
    job: Option<Job>,
    quit: bool,
}

/// Restores the terminal on the way out, however we leave: normal return, `?`,
/// or panic. Without this, an abnormal exit strands the terminal in raw mode
/// inside the alternate screen.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

/// Flag set by SIGINT/SIGTERM/SIGHUP so the event loop can shut down cleanly
/// and let [`TerminalGuard`] run. A second signal takes the default action
/// instead, so a wedged UI can still be killed.
fn install_signal_handlers() -> Result<Arc<AtomicBool>> {
    let shutdown = Arc::new(AtomicBool::new(false));
    for sig in [
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ] {
        signal_hook::flag::register_conditional_shutdown(sig, 1, Arc::clone(&shutdown))?;
        signal_hook::flag::register(sig, Arc::clone(&shutdown))?;
    }
    Ok(shutdown)
}

pub fn run(board: Board) -> Result<()> {
    let shutdown = install_signal_handlers()?;
    let (loop_res, abandoned) = {
        let mut terminal = ratatui::init();
        let _guard = TerminalGuard;
        let mut app = App {
            list_states: COLUMNS.iter().map(|_| ListState::default()).collect(),
            board,
            col: 0,
            mode: Mode::Normal,
            message: String::new(),
            job: None,
            quit: false,
        };
        app.fix_selection();
        let loop_res = event_loop(&mut terminal, &mut app, &shutdown);
        // Teardown runs whatever went wrong — a write to a tty that disappeared
        // mid-agent must not leave the agent's process tree behind us.
        app.cancel_job();
        (loop_res, app.await_cancelled_job())
    };
    // Printed after the guard has restored the terminal, so it lands in the
    // user's shell rather than on the alternate screen we are tearing down —
    // and before the loop's error, which would otherwise hide it.
    if abandoned {
        eprintln!(
            "warning: an agent did not stop in time and was left running; \
             check for stray `opencode`, `git` or `gh` processes"
        );
    }
    loop_res
}

/// Draw and handle input until the user quits. Cancelling and awaiting the
/// running job is the caller's job, so it happens on the error path too.
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    shutdown: &Arc<AtomicBool>,
) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| draw(f, app))?;
        if shutdown.load(Ordering::Relaxed) {
            app.quit = true;
            break;
        }
        if event::poll(TICK)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key.code, key.modifiers);
        }
        app.poll_job();
    }
    Ok(())
}

impl App {
    fn tasks_in_col(&self, col: usize) -> Vec<u32> {
        let pred = COLUMNS[col].1;
        self.board
            .tasks()
            .into_iter()
            .filter(|t| pred(&t.status))
            .map(|t| t.id)
            .collect()
    }

    fn selected_id(&self) -> Option<u32> {
        self.list_states
            .get(self.col)
            .and_then(|ls| ls.selected())
            .and_then(|i| self.tasks_in_col(self.col).get(i).copied())
    }

    fn fix_selection(&mut self) {
        while self.col > 0 && self.tasks_in_col(self.col).is_empty() {
            self.col -= 1;
        }
        for c in 0..COLUMNS.len() {
            let n = self.tasks_in_col(c).len();
            let ls = &mut self.list_states[c];
            if n == 0 {
                ls.select(None);
            } else if ls.selected().is_none_or(|i| i >= n) {
                ls.select(Some(n - 1));
            }
        }
    }

    fn move_col(&mut self, delta: i32) {
        let mut c = self.col as i32 + delta;
        while (0..COLUMNS.len() as i32).contains(&c) && self.tasks_in_col(c as usize).is_empty() {
            c += delta;
        }
        if (0..COLUMNS.len() as i32).contains(&c) {
            self.col = c as usize;
        }
    }

    fn move_row(&mut self, delta: i32) {
        let n = self.tasks_in_col(self.col).len();
        if n == 0 {
            return;
        }
        let ls = &mut self.list_states[self.col];
        let i = ls.selected().map_or(0, |i| i as i32) + delta;
        ls.select(Some(i.clamp(0, n as i32 - 1) as usize));
    }

    fn finish(&mut self, msg: String) {
        self.job = None;
        self.message = msg;
        self.fix_selection();
    }

    /// Hand `action` to a worker thread and show the busy overlay. The worker
    /// opens its own board connection — `rusqlite::Connection` cannot cross
    /// threads — and writes to the same file, which the UI re-reads each frame.
    fn start_job(&mut self, label: String, action: Action) {
        if self.job.is_some() {
            self.message = "an agent is already running".into();
            return;
        }
        let db_path = self.board.db_path.clone();
        let repo_path = self.board.repo_path.clone();
        let (tx, rx) = mpsc::channel();
        agents::reset_cancel();
        std::thread::spawn(move || {
            let res = Board::open(&db_path, repo_path).and_then(|mut board| match action {
                Action::Request(text) => ops::request(&mut board, &text).map(|l| l.join("\n")),
                Action::Run(id) => ops::run_task(&mut board, id),
                Action::Rework(id) => ops::rework(&mut board, id),
            });
            let _ = tx.send(res);
        });
        self.job = Some(Job {
            label,
            rx,
            frame: 0,
            cancelling: false,
            forced: false,
        });
    }

    /// Advance the spinner and collect the worker's result if it has finished.
    fn poll_job(&mut self) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        job.frame = job.frame.wrapping_add(1);
        // Report what the worker actually did, not what we asked it to do: a
        // result may already have been in the channel when Esc was pressed, and
        // labelling a finished `request` "cancelled" invites the user to run it
        // again and duplicate every task it just created.
        let outcome = match job.rx.try_recv() {
            Ok(Ok(msg)) => Some(msg),
            Ok(Err(e)) if agents::was_cancelled(&e) => Some("cancelled".into()),
            Ok(Err(e)) => Some(format!("error: {e:#}")),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some("agent stopped unexpectedly".into()),
        };
        if let Some(msg) = outcome {
            self.finish(msg);
        }
    }

    /// Kill the running agent and the subprocess tree under it.
    fn cancel_job(&mut self) {
        if let Some(job) = self.job.as_mut() {
            if job.cancelling {
                return;
            }
            job.cancelling = true;
            job.label = "cancelling…".into();
            agents::cancel();
        }
    }

    /// Ask to leave immediately: cancel, and skip the grace period a normal
    /// quit would spend waiting for the worker.
    fn force_quit(&mut self) {
        self.cancel_job();
        if let Some(job) = self.job.as_mut() {
            job.forced = true;
        }
        self.quit = true;
    }

    /// Give a cancelled worker a moment to unwind so its board write is not cut
    /// off mid-flight. Bounded, so a stuck agent cannot hold the exit hostage,
    /// and skipped entirely once the user has said to go now.
    ///
    /// Returns true if it did not report back in time — we are about to exit out
    /// from under it, and anything it spawned that survived the kill outlives us.
    fn await_cancelled_job(&mut self) -> bool {
        let Some(job) = self.job.as_ref() else {
            return false;
        };
        if job.forced {
            // It may still finish unwinding after we are gone; say so rather
            // than claiming a clean stop.
            let unfinished = matches!(job.rx.try_recv(), Err(mpsc::TryRecvError::Empty));
            self.job = None;
            return unfinished;
        }
        // A disconnect means the worker thread is gone, which is an orderly
        // enough exit; only a timeout means it is still in there somewhere.
        let abandoned = matches!(
            job.rx.recv_timeout(SHUTDOWN_GRACE),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
        self.job = None;
        abandoned
    }

    /// Start the agent for `id`, if a task is selected.
    fn run_selected(&mut self) {
        if let Some(id) = self.selected_id() {
            let who = self
                .board
                .get(id)
                .and_then(|t| t.assignee.clone())
                .unwrap_or_else(|| "developer".into());
            self.start_job(format!("{who} working on task #{id}…"), Action::Run(id));
        } else {
            self.message = "no task selected".into();
        }
    }

    fn rework_selected(&mut self) {
        if let Some(id) = self.selected_id() {
            self.start_job(
                format!("developer reworking task #{id}…"),
                Action::Rework(id),
            );
        } else {
            self.message = "no task selected".into();
        }
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        let ctrl_c = code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL);
        // While an agent runs, the only meaningful keys are the ones that stop
        // it. A second Ctrl-C leaves without waiting: raw mode swallows SIGINT,
        // so this is the user's only way out of an agent that ignores the kill,
        // and without it the TUI cannot be quit from the keyboard at all.
        // Deliberately not Esc — a double-tap or key repeat would abandon the
        // worker mid-unwind, which is where the board write happens.
        if let Some(job) = self.job.as_ref() {
            if ctrl_c && job.cancelling {
                self.force_quit();
            } else if ctrl_c || code == KeyCode::Esc {
                self.cancel_job();
            }
            return;
        }
        if ctrl_c {
            self.quit = true;
            return;
        }
        match &mut self.mode {
            Mode::Task => {
                self.mode = Mode::Normal;
                match code {
                    KeyCode::Char('r') => self.run_selected(),
                    KeyCode::Char('w') => self.rework_selected(),
                    KeyCode::Char('v') => {
                        self.mode = Mode::Command {
                            prompt: "review: ".into(),
                            input: String::new(),
                        };
                    }
                    _ => {}
                }
            }
            Mode::Help => self.mode = Mode::Normal,
            Mode::Command { prompt, input } => match code {
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Enter => {
                    let cmdline = std::mem::take(input);
                    let prompt_name = prompt.clone();
                    self.mode = Mode::Normal;
                    self.exec_command(&prompt_name, &cmdline);
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => input.push(c),
                _ => {}
            },
            Mode::Normal => match code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Esc => self.mode = Mode::Normal,
                KeyCode::Char('?') | KeyCode::F(1) => self.mode = Mode::Help,
                KeyCode::Left | KeyCode::Char('h') => self.move_col(-1),
                KeyCode::Right | KeyCode::Char('l') => self.move_col(1),
                KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
                KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
                KeyCode::Char(':') => {
                    self.mode = Mode::Command {
                        prompt: "> ".into(),
                        input: String::new(),
                    };
                }
                KeyCode::Char('a') => {
                    self.mode = Mode::Command {
                        prompt: "add: ".into(),
                        input: String::new(),
                    };
                }
                KeyCode::Char('R') => {
                    self.mode = Mode::Command {
                        prompt: "request: ".into(),
                        input: String::new(),
                    };
                }
                KeyCode::Enter => {
                    if self.selected_id().is_some() {
                        self.mode = Mode::Task;
                    } else {
                        self.message = "no task selected".into();
                    }
                }
                KeyCode::Char('r') => self.run_selected(),
                KeyCode::Char('w') => self.rework_selected(),
                KeyCode::Char('v') => {
                    if self.selected_id().is_some() {
                        self.mode = Mode::Command {
                            prompt: "review: ".into(),
                            input: String::new(),
                        };
                    } else {
                        self.message = "no task selected".into();
                    }
                }
                _ => {}
            },
        }
    }

    fn exec_command(&mut self, prompt: &str, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let res: Result<String> = match prompt {
            "review: " => {
                if let Some(id) = self.selected_id() {
                    ops::review(&mut self.board, id, line)
                } else {
                    Err(anyhow::anyhow!("no task selected"))
                }
            }
            "add: " => ops::add_backlog_task(&mut self.board, line),
            "request: " => {
                self.start_job(
                    "scrum master is planning…".into(),
                    Action::Request(line.to_string()),
                );
                return;
            }
            _ => {
                let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
                match cmd {
                    "run" => match rest.parse::<u32>() {
                        Ok(id) => {
                            self.start_job(format!("working on task #{id}…"), Action::Run(id));
                            return;
                        }
                        Err(_) => Err(anyhow::anyhow!("usage: run <id>")),
                    },
                    "rework" => match rest.parse::<u32>() {
                        Ok(id) => {
                            self.start_job(
                                format!("developer reworking task #{id}…"),
                                Action::Rework(id),
                            );
                            return;
                        }
                        Err(_) => Err(anyhow::anyhow!("usage: rework <id>")),
                    },
                    "assign" => match rest.split_once(' ') {
                        Some((id, who)) => match id.parse::<u32>() {
                            Ok(id) => ops::assign(&mut self.board, id, who),
                            Err(_) => Err(anyhow::anyhow!("usage: assign <id> <who>")),
                        },
                        None => Err(anyhow::anyhow!("usage: assign <id> <who>")),
                    },
                    "review" => match rest.split_once(' ') {
                        Some((id, feedback)) => match id.parse::<u32>() {
                            Ok(id) => ops::review(&mut self.board, id, feedback),
                            Err(_) => Err(anyhow::anyhow!("usage: review <id> <feedback>")),
                        },
                        None => Err(anyhow::anyhow!("usage: review <id> <feedback>")),
                    },
                    "quit" => {
                        self.quit = true;
                        return;
                    }
                    other => Err(anyhow::anyhow!("unknown command: {other}")),
                }
            }
        };
        self.message = res.unwrap_or_else(|e| format!("error: {e:#}"));
        self.fix_selection();
    }
}

fn column_color(col: usize) -> Color {
    match col {
        0 => Color::Gray,
        1 => Color::Cyan,
        2 => Color::Yellow,
        3 => Color::Magenta,
        _ => Color::Green,
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let [banner, main, detail, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(8),
        Constraint::Length(2),
    ])
    .areas(f.area());

    draw_banner(f, banner, app);
    let cols = Layout::horizontal(
        std::iter::repeat_n(Constraint::Fill(1), COLUMNS.len())
            .collect::<Vec<_>>(),
    )
    .split(main);
    for (c, (title, _)) in COLUMNS.iter().enumerate() {
        draw_column(f, app, cols[c], c, title);
    }
    draw_detail(f, app, detail);
    draw_footer(f, app, footer);

    if let Some(job) = &app.job {
        draw_busy(f, job);
    }
    if let Mode::Help = app.mode {
        draw_help(f);
    }
    if let Mode::Task = app.mode {
        draw_task_modal(f, app);
    }
}

fn draw_banner(f: &mut Frame, area: Rect, app: &App) {
    let tasks = app.board.tasks();
    let total = tasks.len();
    let done = tasks
        .iter()
        .filter(|t| t.status == Status::Done)
        .count();
    let title = Line::from(vec![
        Span::styled(" ⚒ scrumforge ", Style::default().fg(Color::Black).bg(Color::LightYellow).add_modifier(Modifier::BOLD)),
        Span::raw("  product owner console  "),
        Span::styled(
            format!("{done}/{total} done"),
            Style::default().fg(Color::Green),
        ),
        Span::raw(format!("  · repo {}", app.board.repo_path.display())),
    ]);
    f.render_widget(Paragraph::new(title), area);
}

fn draw_column(f: &mut Frame, app: &mut App, area: Rect, col: usize, title: &str) {
    let color = column_color(col);
    let ids = app.tasks_in_col(col);
    let items: Vec<ListItem> = ids
        .iter()
        .filter_map(|id| app.board.get(*id))
        .map(|t| {
            let assignee = t.assignee.as_deref().unwrap_or("—");
            let marks = if t.pr_url.is_some() { " ⛁" } else { "" };
            ListItem::new(Line::from(vec![
                Span::styled(format!("#{} ", t.id), Style::default().fg(color).bold()),
                Span::raw(format!("{}{}", t.title, marks)),
                Span::styled(format!(" [{assignee}]"), Style::default().fg(Color::DarkGray)),
            ]))
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} ({}) ", ids.len()))
        .border_style(if app.col == col {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        });
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(color).fg(Color::Black).add_modifier(Modifier::BOLD))
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, area, &mut app.list_states[col]);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let body = match app.selected_id().and_then(|id| app.board.get(id)) {
        Some(t) => vec![
            Line::from(vec![
                Span::styled(format!("#{} ", t.id), Style::default().fg(Color::Cyan).bold()),
                Span::styled(t.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(format!("  [{}]", t.status), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(Span::styled(
                if t.description.is_empty() { "(no description)".into() } else { t.description.clone() },
                Style::default().fg(Color::Gray),
            )),
            Line::from(vec![
                Span::styled("assignee: ", Style::default().fg(Color::DarkGray)),
                Span::raw(t.assignee.clone().unwrap_or_else(|| "unassigned".into())),
                Span::styled("   branch: ", Style::default().fg(Color::DarkGray)),
                Span::raw(t.branch.clone().unwrap_or_else(|| "—".into())),
            ]),
            t.pr_url.clone().map(|u| Line::from(vec![
                Span::styled("PR: ", Style::default().fg(Color::DarkGray)),
                Span::styled(u, Style::default().fg(Color::LightBlue).underlined()),
            ])).unwrap_or_default(),
            t.review_notes.clone().filter(|n| !n.is_empty()).map(|n| Line::from(vec![
                Span::styled("review: ", Style::default().fg(Color::DarkGray)),
                Span::raw(n),
            ])).unwrap_or_default(),
        ],
        None => vec![Line::from(Span::styled(
            "no task selected — press R to ask the scrum master for a plan",
            Style::default().fg(Color::DarkGray),
        ))],
    };
    f.render_widget(
        Paragraph::new(body)
            .block(Block::default().borders(Borders::ALL).title(" Task ")),
        area,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    if let Some(job) = &app.job {
        let line = Line::from(vec![
            Span::styled("Esc", Style::default().fg(Color::Red).bold()),
            Span::raw(" or "),
            Span::styled("Ctrl-C", Style::default().fg(Color::Red).bold()),
            Span::raw(if job.cancelling {
                "  │ stopping… Ctrl-C again to quit now"
            } else {
                "  │ cancel the running agent"
            }),
        ]);
        f.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), area);
        return;
    }
    let line = match &app.mode {
        Mode::Command { prompt, input } => Line::from(vec![
            Span::styled(prompt.clone(), Style::default().fg(Color::Cyan).bold()),
            Span::raw(input.clone()),
            Span::styled("▏", Style::default().fg(Color::Cyan)),
        ]),
        _ => Line::from(vec![
            Span::styled("R", Style::default().fg(Color::Green).bold()),
            Span::raw("equest  "),
            Span::styled("a", Style::default().fg(Color::Green).bold()),
            Span::raw("dd  "),
            Span::styled("r", Style::default().fg(Color::Green).bold()),
            Span::raw("un  "),
            // 'w' and 'v' sit mid-word; highlight them in place rather than
            // prefixing, which rendered as "wrework" / "vreview".
            Span::raw("re"),
            Span::styled("w", Style::default().fg(Color::Green).bold()),
            Span::raw("ork  "),
            Span::raw("re"),
            Span::styled("v", Style::default().fg(Color::Green).bold()),
            Span::raw("iew  "),
            Span::styled(":", Style::default().fg(Color::Green).bold()),
            Span::raw("cmd  "),
            Span::styled("?", Style::default().fg(Color::Green).bold()),
            Span::raw("help  "),
            Span::styled("q", Style::default().fg(Color::Red).bold()),
            Span::raw("uit"),
            Span::styled(format!("  │ {}", app.message), Style::default().fg(Color::Yellow)),
        ]),
    };
    f.render_widget(
        Paragraph::new(line).wrap(Wrap { trim: false }),
        area,
    );
}

const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn draw_busy(f: &mut Frame, job: &Job) {
    let frame_char = SPINNER[job.frame % SPINNER.len()];
    // Kept short enough to survive a 60-column terminal: the sizing below
    // clamps the box to the screen, so a longer line loses its tail.
    let hint = if job.cancelling {
        "stopping the agent… Ctrl-C again to quit now"
    } else {
        "agents can take minutes · Esc or Ctrl-C to cancel"
    };
    // Separate `Line`s, not "\n" inside a Span: ratatui renders a Span as a
    // single run of cells, so an embedded newline is swallowed and the tail of
    // the text runs on past the edge of the box.
    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{frame_char} "),
                Style::default().fg(Color::Cyan).bold(),
            ),
            Span::styled(job.label.clone(), Style::default().add_modifier(Modifier::BOLD)),
        ]),
        "".into(),
        Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
    ];

    // Size to the content so the hint can never be clipped mid-sentence.
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 4;
    let height = lines.len() as u16 + 2;
    let area = centered_size(f.area(), width, height);

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .centered()
            .wrap(Wrap { trim: false })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" ⚙ working ")
                    .border_style(Style::default().fg(Color::Cyan)),
            ),
        area,
    );
}

fn draw_task_modal(f: &mut Frame, app: &App) {
    let area = centered(f.area(), 60, 60);
    let task = app.selected_id().and_then(|id| app.board.get(id));
    let body = match task {
        Some(t) => vec![
            Line::from(vec![
                Span::styled(format!("#{} ", t.id), Style::default().fg(Color::Cyan).bold()),
                Span::styled(t.title.clone(), Style::default().add_modifier(Modifier::BOLD)),
            ]),
            Line::from(Span::styled(
                if t.description.is_empty() { "(no description)".into() } else { t.description.clone() },
                Style::default().fg(Color::Gray),
            )),
            "".into(),
            Line::from(vec![
                Span::styled("status:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", t.status), Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("assignee: ", Style::default().fg(Color::DarkGray)),
                Span::raw(t.assignee.clone().unwrap_or_else(|| "unassigned".into())),
            ]),
            Line::from(vec![
                Span::styled("branch:   ", Style::default().fg(Color::DarkGray)),
                Span::raw(t.branch.clone().unwrap_or_else(|| "—".into())),
            ]),
            t.pr_url.clone().map(|u| Line::from(vec![
                Span::styled("PR:       ", Style::default().fg(Color::DarkGray)),
                Span::styled(u, Style::default().fg(Color::LightBlue).underlined()),
            ])).unwrap_or_default(),
            t.review_notes.clone().filter(|n| !n.is_empty()).map(|n| Line::from(vec![
                Span::styled("review:   ", Style::default().fg(Color::DarkGray)),
                Span::raw(n),
            ])).unwrap_or_default(),
            "".into(),
            Line::from(Span::styled(
                "r run · w rework · Esc close",
                Style::default().fg(Color::DarkGray),
            )),
        ],
        None => vec![Line::from("no task selected")],
    };
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(body)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" Task ")),
        area,
    );
}

/// The `?` overlay. Keys, flow, and notes come from [`crate::help`] — the same
/// source `scrumforge help tui` prints — so the two cannot drift apart.
fn draw_help(f: &mut Frame) {
    let heading = |text: &'static str| Line::from(Span::styled(text, Style::default().fg(Color::Cyan).bold()));

    let mut lines = vec![heading("keys")];
    lines.extend(help::KEYS.iter().map(|(key, desc)| {
        Line::from(vec![
            Span::styled(
                format!(" {key:<width$} ", width = help::KEY_WIDTH),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw(*desc),
        ])
    }));

    lines.push("".into());
    lines.push(heading("flow"));
    lines.extend(help::FLOW.iter().map(|l| Line::from(format!(" {l}"))));

    lines.push("".into());
    lines.extend(help::TUI_NOTES.iter().map(|n| {
        Line::from(Span::styled(format!(" {n}"), Style::default().fg(Color::DarkGray)))
    }));

    lines.push("".into());
    lines.push(Line::from(Span::styled(
        " press any key to close · full guide: scrumforge help",
        Style::default().fg(Color::DarkGray),
    )));

    // Size to the content rather than a percentage, so no binding gets wrapped
    // or clipped on a narrow terminal; Wrap is only the fallback for terminals
    // too small even for that.
    let width = lines.iter().map(Line::width).max().unwrap_or(0) as u16 + 2;
    let height = lines.len() as u16 + 2;
    let area = centered_size(f.area(), width, height);

    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::default().borders(Borders::ALL).title(" help ")),
        area,
    );
}

/// A `width` x `height` rect centred in `area`, shrunk to fit if it is bigger.
fn centered_size(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

fn centered(area: Rect, pct_x: u16, pct_y: u16) -> Rect {
    let v = Layout::vertical([Constraint::Percentage(pct_y), Constraint::Percentage(100 - pct_y)]).split(area);
    let h = Layout::horizontal([Constraint::Percentage(pct_x), Constraint::Percentage(100 - pct_x)]).split(v[0].inner(ratatui::layout::Margin::new(0, 0)));
    let inner = h[0];
    let dy = area.height.saturating_sub(inner.height) / 2;
    let dx = area.width.saturating_sub(inner.width) / 2;
    Rect {
        x: area.x + dx,
        y: area.y + dy,
        width: inner.width,
        height: inner.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn render_help(width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(draw_help).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The overlay must show every binding help.rs defines, so adding one there
    /// cannot leave the TUI out of date.
    #[test]
    fn help_overlay_renders_every_key_from_help_module() {
        let rendered = render_help(120, 40);
        for (key, desc) in help::KEYS {
            assert!(rendered.contains(key), "missing key {key} in overlay:\n{rendered}");
            assert!(rendered.contains(desc), "missing desc {desc} in overlay:\n{rendered}");
        }
        for line in help::FLOW {
            assert!(rendered.contains(line), "missing flow line in overlay:\n{rendered}");
        }
    }

    /// Nothing may be truncated at the smallest terminal we expect to run in —
    /// including the trailing notes and footer, which are what a new line added
    /// to `help::KEYS` silently pushes off the bottom.
    #[test]
    fn help_overlay_fits_an_80x24_terminal() {
        let rendered = render_help(80, 24);
        for (_, desc) in help::KEYS {
            assert!(rendered.contains(desc), "truncated at 80x24:\n{rendered}");
        }
        for note in help::TUI_NOTES {
            assert!(rendered.contains(note), "note dropped at 80x24:\n{rendered}");
        }
        assert!(
            rendered.contains("press any key to close"),
            "footer dropped at 80x24:\n{rendered}"
        );
    }

    fn render_busy(job: &Job, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| draw_busy(f, job)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn dummy_job(frame: usize, cancelling: bool) -> Job {
        // The sender is dropped immediately; nothing here polls the channel.
        let (_, rx) = mpsc::channel();
        Job {
            label: "developer working on task #1…".into(),
            rx,
            frame,
            cancelling,
            forced: false,
        }
    }

    /// The smallest terminals we claim to support. Every overlay assertion runs
    /// over all of them — a hint that only fits at 80 columns is a hint the user
    /// loses exactly when they need it.
    const SIZES: [(u16, u16); 3] = [(120, 30), (80, 24), (60, 12)];

    /// The cancel hint used to live in a `"\n(…)"` span, which ratatui renders as
    /// one unbroken run — the sentence ran off the box and was clipped mid-word.
    /// It must appear whole, on its own line, at the sizes we support.
    #[test]
    fn busy_overlay_shows_the_whole_cancel_hint() {
        for (w, h) in SIZES {
            let rendered = render_busy(&dummy_job(0, false), w, h);
            assert!(
                rendered.contains("Esc or Ctrl-C to cancel"),
                "cancel hint clipped at {w}x{h}:\n{rendered}"
            );
            assert!(
                rendered.contains("developer working on task #1…"),
                "label clipped at {w}x{h}:\n{rendered}"
            );
        }
    }

    /// The spinner is driven by the job's frame counter; a fixed frame meant the
    /// overlay looked frozen even while the agent was making progress.
    #[test]
    fn busy_overlay_spinner_advances_with_the_frame_counter() {
        let seen: std::collections::HashSet<char> = (0..SPINNER.len())
            .map(|frame| {
                let rendered = render_busy(&dummy_job(frame, false), 80, 24);
                *SPINNER
                    .iter()
                    .find(|c| rendered.contains(**c))
                    .expect("no spinner glyph rendered")
            })
            .collect();
        assert_eq!(seen.len(), SPINNER.len(), "spinner does not animate");
    }

    /// Cancelling swaps the hint, so the user knows the keypress registered —
    /// and the escape hatch must survive the narrowest terminal, since a wedged
    /// agent is the one situation where it is the only way out.
    #[test]
    fn busy_overlay_reports_cancellation() {
        for (w, h) in SIZES {
            let rendered = render_busy(&dummy_job(0, true), w, h);
            assert!(
                rendered.contains("stopping the agent"),
                "no cancellation feedback at {w}x{h}:\n{rendered}"
            );
            assert!(
                rendered.contains("Ctrl-C again to quit now"),
                "escape hatch clipped at {w}x{h}:\n{rendered}"
            );
        }
    }

    fn test_app() -> App {
        let board = Board::open(std::path::Path::new(":memory:"), "/tmp".into()).unwrap();
        App {
            list_states: COLUMNS.iter().map(|_| ListState::default()).collect(),
            board,
            col: 0,
            mode: Mode::Normal,
            message: String::new(),
            job: None,
            quit: false,
        }
    }

    /// Raw mode swallows SIGINT, so keys are the only way out. A first Ctrl-C
    /// cancels; a second must leave, or an agent that ignores the kill traps the
    /// user in the TUI for good.
    #[test]
    fn second_ctrl_c_quits_even_while_an_agent_runs() {
        // Cancelling reaches process-wide state shared with the agents tests.
        let _serial = agents::test_guard();
        let mut app = test_app();
        app.job = Some(dummy_job(0, false));

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.job.as_ref().is_some_and(|j| j.cancelling), "did not cancel");
        assert!(!app.quit, "quit on the first press");

        app.handle_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit, "did not quit on the second press");
        assert!(
            app.job.as_ref().is_some_and(|j| j.forced),
            "quit without marking the job forced, so exit still waits out the grace period"
        );
    }

    /// Esc cancels but must never force the exit: it is one key, so a repeat or
    /// a double-tap would abandon the worker mid-unwind — and the unwind is
    /// where the board write happens.
    #[test]
    fn repeated_esc_cancels_but_does_not_quit() {
        // Cancelling reaches process-wide state shared with the agents tests.
        let _serial = agents::test_guard();
        let mut app = test_app();
        app.job = Some(dummy_job(0, false));

        for _ in 0..3 {
            app.handle_key(KeyCode::Esc, KeyModifiers::NONE);
        }
        assert!(app.job.as_ref().is_some_and(|j| j.cancelling), "did not cancel");
        assert!(!app.quit, "Esc forced a quit");
    }

    /// A forced quit must not then sit in the grace period it was meant to skip.
    #[test]
    fn forced_quit_does_not_wait_for_the_worker() {
        // Cancelling reaches process-wide state shared with the agents tests.
        let _serial = agents::test_guard();
        let mut app = test_app();
        // Sender held for the whole test: a worker that is still running, which
        // is the only case where the wait would have cost anything.
        let (_tx, rx) = mpsc::channel();
        app.job = Some(Job {
            label: "developer working on task #1…".into(),
            rx,
            frame: 0,
            cancelling: false,
            forced: false,
        });
        app.force_quit();

        let start = std::time::Instant::now();
        let abandoned = app.await_cancelled_job();

        assert!(
            start.elapsed() < SHUTDOWN_GRACE,
            "forced exit still waited {:?}",
            start.elapsed()
        );
        assert!(abandoned, "a worker left running should be reported");
    }

    /// A worker's result can already be in the channel when the user hits Esc.
    /// Reporting "cancelled" there invites re-running an operation that in fact
    /// completed — for `request`, that duplicates every task it created.
    #[test]
    fn a_result_that_beat_the_cancel_is_not_reported_as_cancelled() {
        // Cancelling reaches process-wide state shared with the agents tests.
        let _serial = agents::test_guard();
        let mut app = test_app();
        let (tx, rx) = mpsc::channel();
        app.job = Some(Job {
            label: "scrum master is planning…".into(),
            rx,
            frame: 0,
            cancelling: false,
            forced: false,
        });
        tx.send(Ok("created 3 tasks".into())).unwrap();

        app.cancel_job();
        app.poll_job();

        assert_eq!(app.message, "created 3 tasks");
    }

    /// A genuinely cancelled worker still reads as cancelled, not as an error.
    #[test]
    fn a_cancelled_worker_is_reported_as_cancelled() {
        // Cancelling reaches process-wide state shared with the agents tests.
        let _serial = agents::test_guard();
        let mut app = test_app();
        let (tx, rx) = mpsc::channel();
        app.job = Some(Job {
            label: "developer working on task #1…".into(),
            rx,
            frame: 0,
            cancelling: false,
            forced: false,
        });
        tx.send(Err(anyhow::Error::new(agents::Cancelled).context("spawning opencode")))
            .unwrap();

        app.cancel_job();
        app.poll_job();

        assert_eq!(app.message, "cancelled");
    }
}

