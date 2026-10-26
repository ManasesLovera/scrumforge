use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

use crate::board::{Project, Sprint, Status, Store, Task};
use crate::help;
use crate::ops;

const EMBER: Color = Color::Rgb(251, 146, 60);
const INK: Color = Color::Rgb(234, 227, 216);
const INK_DARK: Color = Color::Rgb(18, 16, 14);
const MUTED: Color = Color::Rgb(167, 156, 142);
const DIM: Color = Color::Rgb(125, 115, 103);
const BACKLOG: Color = Color::Rgb(140, 130, 120);
const TODO: Color = Color::Rgb(192, 168, 122);
const REVIEW: Color = Color::Rgb(224, 138, 43);
const DONE: Color = Color::Rgb(134, 163, 74);
const DANGER: Color = Color::Rgb(248, 113, 113);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Board,
    Backlog,
}

type StatusPred = fn(&Status) -> bool;

const BOARD_COLUMNS: &[(&str, StatusPred)] = &[
    ("Todo", |s| matches!(s, Status::Todo)),
    ("Working", |s| {
        matches!(s, Status::InProgress | Status::ChangesRequested)
    }),
    ("Review", |s| matches!(s, Status::InReview)),
    ("Done", |s| matches!(s, Status::Done)),
];

const BACKLOG_COLUMNS: &[(&str, StatusPred)] = &[("Backlog", |s| matches!(s, Status::Backlog))];

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

struct App {
    store: Store,
    project: Project,
    sprint: Option<Sprint>,
    view: View,
    col: usize,
    list_states: Vec<ListState>,
    mode: Mode,
    message: String,
    busy: Option<String>,
    pending: Option<Action>,
    quit: bool,
}

pub fn run(store: Store, project: Project) -> Result<()> {
    let sprint = store.active_sprint(project.id)?;
    let view = if sprint.is_some() {
        View::Board
    } else {
        View::Backlog
    };
    let mut terminal = ratatui::init();
    let mut app = App {
        list_states: (0..BOARD_COLUMNS.len().max(BACKLOG_COLUMNS.len()))
            .map(|_| ListState::default())
            .collect(),
        store,
        project,
        sprint,
        view,
        col: 0,
        mode: Mode::Normal,
        message: String::new(),
        busy: None,
        pending: None,
        quit: false,
    };
    if app.sprint.is_none() {
        app.message = "no active sprint — showing product backlog".into();
    }
    app.fix_selection();
    let res = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    res
}

fn event_loop(terminal: &mut ratatui::DefaultTerminal, app: &mut App) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| draw(f, app))?;
        if let Some(action) = app.pending.take() {
            app.execute(action);
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            app.handle_key(key.code, key.modifiers);
        }
    }
    Ok(())
}

impl App {
    fn columns(&self) -> &'static [(&'static str, StatusPred)] {
        match self.view {
            View::Board => BOARD_COLUMNS,
            View::Backlog => BACKLOG_COLUMNS,
        }
    }

    fn visible_tasks(&self) -> Vec<Task> {
        let result = match self.view {
            View::Board => match &self.sprint {
                Some(sprint) => self.store.sprint_tasks(self.project.id, sprint.id),
                None => Ok(Vec::new()),
            },
            View::Backlog => self.store.backlog_tasks(self.project.id),
        };
        result.unwrap_or_default()
    }

    fn tasks_in_col(&self, col: usize) -> Vec<u32> {
        let Some((_, pred)) = self.columns().get(col) else {
            return Vec::new();
        };
        self.visible_tasks()
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
        let count = self.columns().len();
        while self.col > 0 && self.tasks_in_col(self.col).is_empty() {
            self.col -= 1;
        }
        for c in 0..count {
            let n = self.tasks_in_col(c).len();
            let ls = &mut self.list_states[c];
            if n == 0 {
                ls.select(None);
            } else if ls.selected().is_none_or(|i| i >= n) {
                ls.select(Some(n - 1));
            }
        }
        if self.col >= count {
            self.col = count.saturating_sub(1);
        }
    }

    fn move_col(&mut self, delta: i32) {
        let count = self.columns().len() as i32;
        let mut c = self.col as i32 + delta;
        while (0..count).contains(&c) && self.tasks_in_col(c as usize).is_empty() {
            c += delta;
        }
        if (0..count).contains(&c) {
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

    fn cycle_view(&mut self) {
        self.view = match self.view {
            View::Board => View::Backlog,
            View::Backlog => View::Board,
        };
        self.col = 0;
        if self.view == View::Board && self.sprint.is_none() {
            self.message = "no active sprint; use `s` to select a sprint".into();
        }
        self.fix_selection();
    }

    fn cycle_sprint(&mut self) {
        let Ok(sprints) = self.store.sprints(self.project.id) else {
            return;
        };
        if sprints.is_empty() {
            self.message = "no sprints yet; create one with `scrumforge sprint add`".into();
            return;
        }
        let current = self
            .sprint
            .as_ref()
            .and_then(|s| sprints.iter().position(|x| x.id == s.id));
        let next = current.map_or(0, |i| (i + 1) % sprints.len());
        self.sprint = Some(sprints[next].clone());
        self.view = View::Board;
        self.col = 0;
        self.message = format!("viewing sprint {}", sprints[next].number);
        self.fix_selection();
    }

    fn start_busy(&mut self, msg: &str) {
        self.busy = Some(msg.to_string());
    }

    fn finish(&mut self, msg: String) {
        self.busy = None;
        self.message = msg;
        let _ = self.sprint.is_none();
        self.fix_selection();
    }

    fn execute(&mut self, action: Action) {
        let actor = "product-owner";
        let res = match action {
            Action::Request(text) => {
                ops::request(&self.store, &self.project, &text, actor).map(|l| l.join("\n"))
            }
            Action::Run(id) => ops::run_task(&self.store, &self.project, id, actor),
            Action::Rework(id) => ops::rework(&self.store, &self.project, id, actor),
        };
        self.finish(res.unwrap_or_else(|e| format!("error: {e:#}")));
    }

    fn handle_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        match &mut self.mode {
            Mode::Task => {
                self.mode = Mode::Normal;
                match code {
                    KeyCode::Char('r') => {
                        if let Some(id) = self.selected_id() {
                            let who = self
                                .store
                                .task(self.project.id, id)
                                .ok()
                                .flatten()
                                .and_then(|t| t.assignee)
                                .unwrap_or_else(|| "developer".into());
                            self.start_busy(&format!("{who} working on task #{id}…"));
                            self.pending = Some(Action::Run(id));
                        }
                    }
                    KeyCode::Char('w') => {
                        if let Some(id) = self.selected_id() {
                            self.start_busy(&format!("developer reworking task #{id}…"));
                            self.pending = Some(Action::Rework(id));
                        }
                    }
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
                KeyCode::Char('b') => self.cycle_view(),
                KeyCode::Char('s') => self.cycle_sprint(),
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
                KeyCode::Char('r') => {
                    if let Some(id) = self.selected_id() {
                        let who = self
                            .store
                            .task(self.project.id, id)
                            .ok()
                            .flatten()
                            .and_then(|t| t.assignee)
                            .unwrap_or_else(|| "developer".into());
                        self.start_busy(&format!("{who} working on task #{id}…"));
                        self.pending = Some(Action::Run(id));
                    } else {
                        self.message = "no task selected".into();
                    }
                }
                KeyCode::Char('w') => {
                    if let Some(id) = self.selected_id() {
                        self.start_busy(&format!("developer reworking task #{id}…"));
                        self.pending = Some(Action::Rework(id));
                    }
                }
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
        let actor = "product-owner";
        let res: Result<String> = match prompt {
            "review: " => {
                if let Some(id) = self.selected_id() {
                    ops::review(&self.store, &self.project, id, line, actor)
                } else {
                    Err(anyhow::anyhow!("no task selected"))
                }
            }
            "add: " => ops::add_backlog_task(&self.store, &self.project, line, None, actor),
            "request: " => {
                self.start_busy("planning…");
                self.pending = Some(Action::Request(line.to_string()));
                return;
            }
            _ => {
                let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
                match cmd {
                    "run" => match rest.parse::<u32>() {
                        Ok(id) => {
                            self.start_busy(&format!("working on task #{id}…"));
                            self.pending = Some(Action::Run(id));
                            return;
                        }
                        Err(_) => Err(anyhow::anyhow!("usage: run <id>")),
                    },
                    "rework" => match rest.parse::<u32>() {
                        Ok(id) => {
                            self.start_busy(&format!("developer reworking task #{id}…"));
                            self.pending = Some(Action::Rework(id));
                            return;
                        }
                        Err(_) => Err(anyhow::anyhow!("usage: rework <id>")),
                    },
                    "assign" => match rest.split_once(' ') {
                        Some((id, who)) => match id.parse::<u32>() {
                            Ok(id) => ops::assign(&self.store, &self.project, id, who, actor),
                            Err(_) => Err(anyhow::anyhow!("usage: assign <id> <who>")),
                        },
                        None => Err(anyhow::anyhow!("usage: assign <id> <who>")),
                    },
                    "review" => match rest.split_once(' ') {
                        Some((id, feedback)) => match id.parse::<u32>() {
                            Ok(id) => ops::review(&self.store, &self.project, id, feedback, actor),
                            Err(_) => Err(anyhow::anyhow!("usage: review <id> <feedback>")),
                        },
                        None => Err(anyhow::anyhow!("usage: review <id> <feedback>")),
                    },
                    "sprint" => {
                        let (sub, arg) = rest.split_once(' ').unwrap_or((rest, ""));
                        match sub {
                            "activate" => match arg.parse::<u32>() {
                                Ok(number) => self
                                    .store
                                    .activate_sprint(self.project.id, number)
                                    .map(|s| format!("activated sprint {}", s.number)),
                                Err(_) => Err(anyhow::anyhow!("usage: sprint activate <number>")),
                            },
                            "close" => {
                                let mut parts = arg.split_whitespace();
                                let number = parts.next().and_then(|v| v.parse::<u32>().ok());
                                match number {
                                    Some(number) => self
                                        .store
                                        .close_sprint(self.project.id, number, parts.next())
                                        .map(|s| format!("closed sprint {}", s.number)),
                                    None => Err(anyhow::anyhow!(
                                        "usage: sprint close <number> [backlog]"
                                    )),
                                }
                            }
                            _ => Err(anyhow::anyhow!("usage: sprint activate|close")),
                        }
                    }
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

fn column_color(col: usize, view: View) -> Color {
    match (view, col) {
        (View::Backlog, _) => BACKLOG,
        (_, 0) => TODO,
        (_, 1) => EMBER,
        (_, 2) => REVIEW,
        _ => DONE,
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let [banner, main, detail, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(9),
        Constraint::Length(2),
    ])
    .areas(f.area());

    draw_banner(f, banner, app);
    let columns = app.columns();
    let cols = Layout::horizontal(
        std::iter::repeat_n(Constraint::Fill(1), columns.len()).collect::<Vec<_>>(),
    )
    .split(main);
    for (c, (title, _)) in columns.iter().enumerate() {
        draw_column(f, app, cols[c], c, title);
    }
    draw_detail(f, app, detail);
    draw_footer(f, app, footer);

    if let Some(busy) = &app.busy {
        draw_busy(f, busy);
    }
    if let Mode::Help = app.mode {
        draw_help(f);
    }
    if let Mode::Task = app.mode {
        draw_task_modal(f, app);
    }
}

fn draw_banner(f: &mut Frame, area: Rect, app: &App) {
    let tasks = app.visible_tasks();
    let total = tasks.len();
    let done = tasks.iter().filter(|t| t.status == Status::Done).count();
    let sprint = app
        .sprint
        .as_ref()
        .map(|s| format!("sprint {} {} · {}", s.number, s.name, s.status))
        .unwrap_or_else(|| "no sprint".into());
    let view = match app.view {
        View::Board => "board",
        View::Backlog => "backlog",
    };
    let title = Line::from(vec![
        Span::styled(
            " scrumforge ",
            Style::default()
                .fg(INK_DARK)
                .bg(EMBER)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ({}) ", app.project.route, app.project.methodology),
            Style::default().fg(MUTED),
        ),
        Span::styled(format!("{view} · {sprint}"), Style::default().fg(INK)),
        Span::styled(format!("  {done}/{total} done"), Style::default().fg(DONE)),
        Span::styled(
            format!("  · {}", app.project.repo_path.display()),
            Style::default().fg(DIM),
        ),
    ]);
    f.render_widget(Paragraph::new(title), area);
}

fn draw_column(f: &mut Frame, app: &mut App, area: Rect, col: usize, title: &str) {
    let color = column_color(col, app.view);
    let ids = app.tasks_in_col(col);
    let items: Vec<ListItem> = ids
        .iter()
        .filter_map(|id| app.store.task(app.project.id, *id).ok().flatten())
        .map(|t| {
            let assignee = t.assignee.as_deref().unwrap_or("—");
            let spec = t
                .spec_slug
                .as_deref()
                .map(|s| format!(" ◇{s}"))
                .unwrap_or_default();
            let marks = if t.pr_url.is_some() { " ⛁" } else { "" };
            ListItem::new(Line::from(vec![
                Span::styled(format!("#{} ", t.id), Style::default().fg(color).bold()),
                Span::raw(format!("{}{}{}", t.title, spec, marks)),
                Span::styled(format!(" [{assignee}]"), Style::default().fg(DIM)),
            ]))
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} ({}) ", ids.len()))
        .border_style(if app.col == col {
            Style::default().fg(color).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(DIM)
        });
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(color)
                .fg(INK_DARK)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    f.render_stateful_widget(list, area, &mut app.list_states[col]);
}

fn draw_detail(f: &mut Frame, app: &App, area: Rect) {
    let body = match app
        .selected_id()
        .and_then(|id| app.store.task(app.project.id, id).ok().flatten())
    {
        Some(t) => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(format!("#{} ", t.id), Style::default().fg(EMBER).bold()),
                    Span::styled(
                        t.title.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  [{}]", t.status), Style::default().fg(REVIEW)),
                ]),
                Line::from(Span::styled(
                    if t.description.is_empty() {
                        "(no description)".into()
                    } else {
                        t.description.clone()
                    },
                    Style::default().fg(INK),
                )),
            ];
            if !t.acceptance_criteria.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("acceptance: ", Style::default().fg(DIM)),
                    Span::raw(t.acceptance_criteria.clone()),
                ]));
            }
            lines.push(Line::from(vec![
                Span::styled("assignee: ", Style::default().fg(DIM)),
                Span::raw(t.assignee.clone().unwrap_or_else(|| "unassigned".into())),
                Span::styled("   branch: ", Style::default().fg(DIM)),
                Span::raw(t.branch.clone().unwrap_or_else(|| "—".into())),
            ]));
            if let Some(spec) = t.spec_slug {
                lines.push(Line::from(vec![
                    Span::styled("spec: ", Style::default().fg(DIM)),
                    Span::raw(spec),
                    Span::styled(
                        t.requirement_key
                            .map(|k| format!("  requirement: {k}"))
                            .unwrap_or_default(),
                        Style::default().fg(DIM),
                    ),
                ]));
            }
            if let Some(u) = t.pr_url {
                lines.push(Line::from(vec![
                    Span::styled("PR: ", Style::default().fg(DIM)),
                    Span::styled(u, Style::default().fg(EMBER).underlined()),
                ]));
            }
            if let Some(n) = t.review_notes.filter(|n| !n.is_empty()) {
                lines.push(Line::from(vec![
                    Span::styled("review: ", Style::default().fg(DIM)),
                    Span::raw(n),
                ]));
            }
            lines
        }
        None => vec![Line::from(Span::styled(
            "no task selected — press R to plan, a to add, b for backlog",
            Style::default().fg(DIM),
        ))],
    };
    f.render_widget(
        Paragraph::new(body).block(Block::default().borders(Borders::ALL).title(" Task ")),
        area,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = match &app.mode {
        Mode::Command { prompt, input } => Line::from(vec![
            Span::styled(prompt.clone(), Style::default().fg(EMBER).bold()),
            Span::raw(input.clone()),
            Span::styled("▏", Style::default().fg(EMBER)),
        ]),
        _ => Line::from(vec![
            Span::styled("R", Style::default().fg(EMBER).bold()),
            Span::raw("equest  "),
            Span::styled("a", Style::default().fg(EMBER).bold()),
            Span::raw("dd  "),
            Span::styled("r", Style::default().fg(EMBER).bold()),
            Span::raw("un  "),
            Span::styled("w", Style::default().fg(EMBER).bold()),
            Span::raw("rework  "),
            Span::styled("v", Style::default().fg(EMBER).bold()),
            Span::raw("review  "),
            Span::styled("b", Style::default().fg(EMBER).bold()),
            Span::raw("acklog  "),
            Span::styled("s", Style::default().fg(EMBER).bold()),
            Span::raw("print  "),
            Span::styled(":", Style::default().fg(EMBER).bold()),
            Span::raw("cmd  "),
            Span::styled("?", Style::default().fg(EMBER).bold()),
            Span::raw("help  "),
            Span::styled("q", Style::default().fg(DANGER).bold()),
            Span::raw("uit"),
            Span::styled(format!("  │ {}", app.message), Style::default().fg(REVIEW)),
        ]),
    };
    f.render_widget(Paragraph::new(line).wrap(Wrap { trim: false }), area);
}

fn draw_busy(f: &mut Frame, msg: &str) {
    let area = centered(f.area(), 50, 20);
    let text = Line::from(vec![
        Span::styled("⠋ ", Style::default().fg(EMBER).bold()),
        Span::styled(msg, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            "\n(agents can take minutes — Ctrl-C aborts)",
            Style::default().fg(DIM),
        ),
    ]);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text).centered().block(
            Block::default()
                .borders(Borders::ALL)
                .title(" working ")
                .border_style(Style::default().fg(EMBER)),
        ),
        area,
    );
}

fn draw_task_modal(f: &mut Frame, app: &App) {
    let area = centered(f.area(), 60, 60);
    let task = app
        .selected_id()
        .and_then(|id| app.store.task(app.project.id, id).ok().flatten());
    let body = match task {
        Some(t) => vec![
            Line::from(vec![
                Span::styled(format!("#{} ", t.id), Style::default().fg(EMBER).bold()),
                Span::styled(
                    t.title.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                if t.description.is_empty() {
                    "(no description)".into()
                } else {
                    t.description.clone()
                },
                Style::default().fg(INK),
            )),
            "".into(),
            Line::from(vec![
                Span::styled("status:   ", Style::default().fg(DIM)),
                Span::styled(format!("{}", t.status), Style::default().fg(REVIEW)),
            ]),
            Line::from(vec![
                Span::styled("assignee: ", Style::default().fg(DIM)),
                Span::raw(t.assignee.clone().unwrap_or_else(|| "unassigned".into())),
            ]),
            Line::from(vec![
                Span::styled("branch:   ", Style::default().fg(DIM)),
                Span::raw(t.branch.clone().unwrap_or_else(|| "—".into())),
            ]),
            t.pr_url
                .clone()
                .map(|u| {
                    Line::from(vec![
                        Span::styled("PR:       ", Style::default().fg(DIM)),
                        Span::styled(u, Style::default().fg(EMBER).underlined()),
                    ])
                })
                .unwrap_or_default(),
            t.review_notes
                .clone()
                .filter(|n| !n.is_empty())
                .map(|n| {
                    Line::from(vec![
                        Span::styled("review:   ", Style::default().fg(DIM)),
                        Span::raw(n),
                    ])
                })
                .unwrap_or_default(),
            "".into(),
            Line::from(Span::styled(
                "r run · w rework · Esc close",
                Style::default().fg(DIM),
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

fn draw_help(f: &mut Frame) {
    let heading =
        |text: &'static str| Line::from(Span::styled(text, Style::default().fg(EMBER).bold()));

    let mut lines = vec![heading("keys")];
    lines.extend(help::KEYS.iter().map(|(key, desc)| {
        Line::from(vec![
            Span::styled(
                format!(" {key:<width$} ", width = help::KEY_WIDTH),
                Style::default().fg(REVIEW),
            ),
            Span::raw(*desc),
        ])
    }));

    lines.push("".into());
    lines.push(heading("flow"));
    lines.extend(help::FLOW.iter().map(|l| Line::from(format!(" {l}"))));

    lines.push("".into());
    lines.extend(
        help::TUI_NOTES
            .iter()
            .map(|n| Line::from(Span::styled(format!(" {n}"), Style::default().fg(DIM)))),
    );

    lines.push("".into());
    lines.push(Line::from(Span::styled(
        " press any key to close · full guide: scrumforge help",
        Style::default().fg(DIM),
    )));

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
    let v = Layout::vertical([
        Constraint::Percentage(pct_y),
        Constraint::Percentage(100 - pct_y),
    ])
    .split(area);
    let h = Layout::horizontal([
        Constraint::Percentage(pct_x),
        Constraint::Percentage(100 - pct_x),
    ])
    .split(v[0].inner(ratatui::layout::Margin::new(0, 0)));
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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

    #[test]
    fn help_overlay_renders_every_key_from_help_module() {
        let rendered = render_help(120, 40);
        for (key, desc) in help::KEYS {
            assert!(
                rendered.contains(key),
                "missing key {key} in overlay:\n{rendered}"
            );
            assert!(
                rendered.contains(desc),
                "missing desc {desc} in overlay:\n{rendered}"
            );
        }
        for line in help::FLOW {
            assert!(
                rendered.contains(line),
                "missing flow line in overlay:\n{rendered}"
            );
        }
    }

    #[test]
    fn help_overlay_fits_an_80x24_terminal() {
        let rendered = render_help(80, 24);
        for (_, desc) in help::KEYS {
            assert!(rendered.contains(desc), "truncated at 80x24:\n{rendered}");
        }
    }
}
