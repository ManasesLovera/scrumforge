use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::board::{Board, Status};
use crate::ops;

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

struct App {
    board: Board,
    col: usize,
    list_states: Vec<ListState>,
    mode: Mode,
    message: String,
    busy: Option<String>,
    pending: Option<Action>,
    quit: bool,
}

pub fn run(board: Board) -> Result<()> {
    let mut terminal = ratatui::init();
    let mut app = App {
        list_states: COLUMNS.iter().map(|_| ListState::default()).collect(),
        board,
        col: 0,
        mode: Mode::Normal,
        message: String::new(),
        busy: None,
        pending: None,
        quit: false,
    };
    app.fix_selection();
    let res = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    res
}

fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
) -> Result<()> {
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

    fn start_busy(&mut self, msg: &str) {
        self.busy = Some(msg.to_string());
    }

    fn finish(&mut self, msg: String) {
        self.busy = None;
        self.message = msg;
        self.fix_selection();
    }

    fn execute(&mut self, action: Action) {
        let res = match action {
            Action::Request(text) => ops::request(&mut self.board, &text).map(|l| l.join("\n")),
            Action::Run(id) => ops::run_task(&mut self.board, id),
            Action::Rework(id) => ops::rework(&mut self.board, id),
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
                                .board
                                .get(id)
                                .and_then(|t| t.assignee.clone())
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
                            .board
                            .get(id)
                            .and_then(|t| t.assignee.clone())
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
                self.start_busy("scrum master is planning…");
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
            Span::styled("w", Style::default().fg(Color::Green).bold()),
            Span::raw("rework  "),
            Span::styled("v", Style::default().fg(Color::Green).bold()),
            Span::raw("review  "),
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

fn draw_busy(f: &mut Frame, msg: &str) {
    let area = centered(f.area(), 50, 20);
    let spinner = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏";
    let frame_char = spinner.chars().next().unwrap_or('*');
    let text = Line::from(vec![
        Span::styled(format!("{frame_char} "), Style::default().fg(Color::Cyan).bold()),
        Span::styled(msg, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled("\n(agents can take minutes — Ctrl-C aborts)", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(text)
            .centered()
            .block(Block::default().borders(Borders::ALL).title(" ⚙ working ").border_style(Style::default().fg(Color::Cyan))),
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

fn draw_help(f: &mut Frame) {
    let area = centered(f.area(), 60, 50);
    let help = vec![
        Line::from(Span::styled("keys", Style::default().fg(Color::Cyan).bold())),
        " ←/h →/l   select column".into(),
        " ↑/k ↓/j   select task".into(),
        " Enter     open selected task (Esc closes)".into(),
        " r         send task to its assignee".into(),
        " w         developer reworks after changes requested".into(),
        " v         review: send task back in progress with feedback".into(),
        " R         ask scrum master to plan a request".into(),
        " a         add a backlog task (\"title\" | \"desc\")".into(),
        " :         command mode (run/rework/assign/quit…)".into(),
        " q         quit (Esc closes modals)".into(),
        "".into(),
        Line::from(Span::styled("flow", Style::default().fg(Color::Cyan).bold())),
        " request → assigned → run → in-review → run (reviewer)".into(),
        "   → merged ✓   or changes-requested → w → in-review …".into(),
        "".into(),
        Line::from(Span::styled("press any key to close", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(help)
            .block(Block::default().borders(Borders::ALL).title(" help ")),
        area,
    );
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
