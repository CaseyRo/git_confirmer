use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};
use ratatui::Terminal;
use walkdir::WalkDir;

use serde::Deserialize;
use serde_json::json;

#[derive(Clone, Debug)]
struct Repo {
    path: PathBuf,
    name: String,
    branch: String,
    ahead: i32,
    behind: i32,
    staged: usize,
    unstaged: usize,
    untracked: usize,
    selected: bool,
    comment: String,
    gen_state: GenState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Normal,
    EditingComment,
}

#[derive(Clone, Copy, Debug)]
struct Theme {
    background: Color,
    foreground: Color,
    header_bg: Color,
    header_fg: Color,
    highlight_bg: Color,
    highlight_fg: Color,
    dirty_fg: Color,
    clean_fg: Color,
    status_fg: Color,
    error_fg: Color,
    border_fg: Color,
}

#[derive(Clone, Debug)]
enum GenState {
    Idle,
    Pending,
    Running,
    Done,
    Error(String),
}

#[derive(Debug)]
enum GenEvent {
    Started(PathBuf),
    Finished(PathBuf, String),
    Failed(PathBuf, String),
    AllDone,
}

#[derive(Clone, Debug)]
struct GenConfig {
    api_key: String,
    model: String,
    max_tokens: u32,
    timeout_secs: u64,
}

struct App {
    repos: Vec<Repo>,
    table_state: TableState,
    mode: Mode,
    input: String,
    status: String,
    base_message: String,
    roots: Vec<PathBuf>,
    theme: Theme,
    gen_active: bool,
    gen_total: usize,
    gen_done: usize,
    gen_errors: usize,
    gen_receiver: Option<Receiver<GenEvent>>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct FileConfig {
    roots: Option<Vec<String>>,
    base_message: Option<String>,
    theme_path: Option<String>,
}

impl App {
    fn new(roots: Vec<PathBuf>, base_message: String, theme_path: Option<PathBuf>) -> io::Result<Self> {
        let repos = scan_repos(&roots)?;
        let theme = load_theme(theme_path).unwrap_or_else(|_| default_theme());
        let mut table_state = TableState::default();
        if !repos.is_empty() {
            table_state.select(Some(0));
        }

        Ok(Self {
            repos,
            table_state,
            mode: Mode::Normal,
            input: String::new(),
            status: String::from("Ready"),
            base_message,
            roots,
            theme,
            gen_active: false,
            gen_total: 0,
            gen_done: 0,
            gen_errors: 0,
            gen_receiver: None,
        })
    }

    fn selected_index(&self) -> Option<usize> {
        self.table_state.selected()
    }

    fn move_selection(&mut self, delta: i32) {
        if self.repos.is_empty() {
            self.table_state.select(None);
            return;
        }
        let max = self.repos.len() as i32 - 1;
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, max) as usize;
        self.table_state.select(Some(next));
    }

    fn toggle_selected(&mut self) {
        if let Some(idx) = self.selected_index() {
            if let Some(repo) = self.repos.get_mut(idx) {
                repo.selected = !repo.selected;
            }
        }
    }

    fn select_dirty(&mut self) {
        for repo in &mut self.repos {
            repo.selected = repo.staged + repo.unstaged + repo.untracked > 0;
        }
    }

    fn clear_selection(&mut self) {
        for repo in &mut self.repos {
            repo.selected = false;
        }
    }

    fn begin_edit_comment(&mut self) {
        if let Some(idx) = self.selected_index() {
            if let Some(repo) = self.repos.get(idx) {
                self.input = repo.comment.clone();
                self.mode = Mode::EditingComment;
            }
        }
    }

    fn save_comment(&mut self) {
        if let Some(idx) = self.selected_index() {
            if let Some(repo) = self.repos.get_mut(idx) {
                repo.comment = self.input.trim().to_string();
                self.input.clear();
                self.mode = Mode::Normal;
            }
        }
    }

    fn cancel_comment(&mut self) {
        self.input.clear();
        self.mode = Mode::Normal;
    }

    fn rescan(&mut self) -> io::Result<()> {
        let mut sticky: HashMap<PathBuf, (bool, String)> = HashMap::new();
        for repo in &self.repos {
            sticky.insert(repo.path.clone(), (repo.selected, repo.comment.clone()));
        }
        self.repos = scan_repos(&self.roots)?;
        for repo in &mut self.repos {
            if let Some((selected, comment)) = sticky.get(&repo.path) {
                repo.selected = *selected;
                repo.comment = comment.clone();
            }
            repo.gen_state = GenState::Idle;
        }
        if self.repos.is_empty() {
            self.table_state.select(None);
        } else if self.table_state.selected().is_none() {
            self.table_state.select(Some(0));
        }
        Ok(())
    }

    fn commit_selected(&mut self) {
        let mut committed = 0;
        let mut skipped = 0;
        let mut errors = 0;
        let mut last_error = String::new();

        for repo in &self.repos {
            if !repo.selected {
                continue;
            }

            match has_changes(&repo.path) {
                Ok(false) => {
                    skipped += 1;
                    continue;
                }
                Err(err) => {
                    errors += 1;
                    last_error = format!("{}: {}", repo.name, err);
                    continue;
                }
                Ok(true) => {}
            }

            if let Err(err) = run_git(&repo.path, ["add", "-A"]) {
                errors += 1;
                last_error = format!("{}: add failed ({})", repo.name, err);
                continue;
            }

            let mut message = self.base_message.clone();
            if !repo.comment.is_empty() {
                message = format!("{} - {}", message, repo.comment);
            }

            if let Err(err) = run_git(&repo.path, ["commit", "-m", &message]) {
                errors += 1;
                last_error = format!("{}: commit failed ({})", repo.name, err);
                continue;
            }

            committed += 1;
        }

        if let Err(err) = self.rescan() {
            self.status = format!("Rescan failed: {}", err);
        } else if errors > 0 {
            self.status = format!(
                "Committed {}, skipped {}, errors {}. Last error: {}",
                committed, skipped, errors, last_error
            );
        } else {
            self.status = format!("Committed {}, skipped {}.", committed, skipped);
        }
    }

    fn start_generation(&mut self) {
        if self.gen_active {
            self.status = String::from("Generation already running.");
            return;
        }

        let api_key = match std::env::var("OPENAI_API_KEY") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                self.status = String::from("OPENAI_API_KEY not set.");
                return;
            }
        };

        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| String::from("gpt-4o-mini"));
        let max_tokens = std::env::var("OPENAI_MAX_TOKENS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(32);
        let timeout_secs = std::env::var("OPENAI_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(10);

        let config = GenConfig {
            api_key,
            model,
            max_tokens,
            timeout_secs,
        };

        let targets: Vec<PathBuf> = self
            .repos
            .iter()
            .filter(|repo| repo.selected && (repo.staged + repo.unstaged + repo.untracked > 0))
            .map(|repo| repo.path.clone())
            .collect();

        if targets.is_empty() {
            self.status = String::from("No selected repos with changes.");
            return;
        }

        for repo in &mut self.repos {
            if repo.selected && (repo.staged + repo.unstaged + repo.untracked > 0) {
                repo.gen_state = GenState::Pending;
            }
        }

        let (tx, rx) = mpsc::channel();
        self.gen_receiver = Some(rx);
        self.gen_active = true;
        self.gen_total = targets.len();
        self.gen_done = 0;
        self.gen_errors = 0;
        self.status = format!("Generating 0/{}...", self.gen_total);

        thread::spawn(move || {
            for path in targets {
                let _ = tx.send(GenEvent::Started(path.clone()));
                match generate_commit_message(&path, &config) {
                    Ok(message) => {
                        let _ = tx.send(GenEvent::Finished(path.clone(), message));
                    }
                    Err(err) => {
                        let _ = tx.send(GenEvent::Failed(path.clone(), err.to_string()));
                    }
                }
            }
            let _ = tx.send(GenEvent::AllDone);
        });
    }

    fn process_gen_events(&mut self) {
        loop {
            let event = match self.gen_receiver.as_ref() {
                Some(rx) => match rx.try_recv() {
                    Ok(event) => event,
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.gen_active = false;
                        self.gen_receiver = None;
                        self.status = String::from("Generation channel closed.");
                        break;
                    }
                },
                None => break,
            };

            match event {
                GenEvent::Started(path) => {
                    if let Some(repo) = self.repos.iter_mut().find(|repo| repo.path == path) {
                        repo.gen_state = GenState::Running;
                    }
                }
                GenEvent::Finished(path, message) => {
                    if let Some(repo) = self.repos.iter_mut().find(|repo| repo.path == path) {
                        repo.comment = message;
                        repo.gen_state = GenState::Done;
                    }
                    self.gen_done += 1;
                }
                GenEvent::Failed(path, error) => {
                    if let Some(repo) = self.repos.iter_mut().find(|repo| repo.path == path) {
                        repo.gen_state = GenState::Error(error);
                    }
                    self.gen_done += 1;
                    self.gen_errors += 1;
                }
                GenEvent::AllDone => {
                    self.gen_active = false;
                    self.gen_receiver = None;
                    if self.gen_errors > 0 {
                        self.status = format!(
                            "Generated {}/{} with {} errors.",
                            self.gen_total - self.gen_errors,
                            self.gen_total,
                            self.gen_errors
                        );
                    } else {
                        self.status = format!("Generated {}/{}.", self.gen_total, self.gen_total);
                    }
                }
            }

            if self.gen_active {
                self.status = format!("Generating {}/{}...", self.gen_done, self.gen_total);
            }
        }
    }
}

fn main() -> io::Result<()> {
    let (roots, base_message, config) = parse_args()?;
    let theme_path = config.theme_path.map(|value| expand_tilde(&value));
    let mut app = App::new(roots, base_message, theme_path)?;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui(&mut terminal, &mut app);

    disable_raw_mode()?;
    terminal.backend_mut().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_tui(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        app.process_gen_events();
        terminal.draw(|frame| render(frame, app))?;

        if event::poll(std::time::Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if handle_key(app, key)? {
                    return Ok(());
                }
            }
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> io::Result<bool> {
    match app.mode {
        Mode::Normal => match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Up => app.move_selection(-1),
            KeyCode::Down => app.move_selection(1),
            KeyCode::Char('k') => app.move_selection(-1),
            KeyCode::Char('j') => app.move_selection(1),
            KeyCode::Char(' ') => app.toggle_selected(),
            KeyCode::Char('a') => app.select_dirty(),
            KeyCode::Char('n') => app.clear_selection(),
            KeyCode::Char('e') => app.begin_edit_comment(),
            KeyCode::Char('g') => app.start_generation(),
            KeyCode::Char('c') => app.commit_selected(),
            KeyCode::Char('r') => {
                if app.gen_active {
                    app.status = String::from("Generation running; wait to rescan.");
                } else {
                    app.rescan()?;
                }
            }
            _ => {}
        },
        Mode::EditingComment => match key.code {
            KeyCode::Esc => app.cancel_comment(),
            KeyCode::Enter => app.save_comment(),
            KeyCode::Backspace => {
                app.input.pop();
            }
            KeyCode::Char(c) => {
                if key.modifiers.contains(KeyModifiers::CONTROL) {
                    return Ok(false);
                }
                app.input.push(c);
            }
            _ => {}
        },
    }

    Ok(false)
}

fn render(frame: &mut ratatui::Frame, app: &mut App) {
    let theme = app.theme;
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(4)])
        .split(frame.size());

    let header = Row::new(vec![
        Cell::from("Sel"),
        Cell::from("Repo"),
        Cell::from("Branch"),
        Cell::from("A/B"),
        Cell::from("Stg"),
        Cell::from("Wk"),
        Cell::from("Untrk"),
        Cell::from("Path"),
        Cell::from("Comment"),
    ])
    .style(
        Style::default()
            .fg(theme.header_fg)
            .bg(theme.header_bg)
            .add_modifier(Modifier::BOLD),
    );

    let rows = app.repos.iter().map(|repo| {
        let dirty = repo.staged + repo.unstaged + repo.untracked > 0;
        let mut style = if dirty {
            Style::default().fg(theme.dirty_fg)
        } else {
            Style::default().fg(theme.clean_fg)
        };

        let sel = if repo.selected { "[x]" } else { "[ ]" };
        let ahead_behind = format!("+{} -{}", repo.ahead, repo.behind);
        let comment = match &repo.gen_state {
            GenState::Pending | GenState::Running => String::from("gen..."),
            GenState::Error(err) => truncate_for_prompt(&format!("err: {}", err), 40),
            GenState::Done | GenState::Idle => repo.comment.clone(),
        };

        if matches!(repo.gen_state, GenState::Error(_)) {
            style = Style::default().fg(theme.error_fg);
        }

        Row::new(vec![
            Cell::from(sel),
            Cell::from(repo.name.clone()),
            Cell::from(repo.branch.clone()),
            Cell::from(ahead_behind),
            Cell::from(repo.staged.to_string()),
            Cell::from(repo.unstaged.to_string()),
            Cell::from(repo.untracked.to_string()),
            Cell::from(repo.path.display().to_string()),
            Cell::from(comment),
        ])
        .style(style)
    });

    let table = Table::new(rows, [
        Constraint::Length(5),
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(10),
        Constraint::Length(5),
        Constraint::Length(5),
        Constraint::Length(7),
        Constraint::Min(40),
        Constraint::Min(24),
    ])
    .header(header)
    .block(
        Block::default()
            .title("Git Confirmer")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border_fg).bg(theme.background)),
    )
    .style(Style::default().fg(theme.foreground).bg(theme.background))
    .highlight_style(
        Style::default()
            .fg(theme.highlight_fg)
            .bg(theme.highlight_bg)
            .add_modifier(Modifier::BOLD),
    );

    frame.render_stateful_widget(table, layout[0], &mut app.table_state);

    let footer = match app.mode {
        Mode::Normal => Line::from(vec![
            Span::raw("q quit  "),
            Span::raw("up/down or j/k move  "),
            Span::raw("space toggle  "),
            Span::raw("a select dirty  "),
            Span::raw("n clear  "),
            Span::raw("e comment  "),
            Span::raw("g generate  "),
            Span::raw("c commit  "),
            Span::raw("r rescan"),
        ]),
        Mode::EditingComment => Line::from(Span::raw("Editing comment: enter to save, esc to cancel")),
    };

    let status_text = if app.mode == Mode::EditingComment {
        format!("Comment: {}", app.input)
    } else {
        app.status.clone()
    };
    let status_line = Line::from(Span::styled(
        status_text,
        Style::default().fg(theme.status_fg).bg(theme.background),
    ));

    let footer_block = Paragraph::new(vec![status_line, footer])
        .style(Style::default().fg(theme.foreground).bg(theme.background))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Status")
                .border_style(Style::default().fg(theme.border_fg).bg(theme.background)),
        );

    frame.render_widget(footer_block, layout[1]);
}

fn parse_args() -> io::Result<(Vec<PathBuf>, String, FileConfig)> {
    let config = load_config().unwrap_or_default();
    let mut args = std::env::args().skip(1).peekable();
    let mut roots: Vec<PathBuf> = config
        .roots
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|value| expand_tilde(&value))
        .collect();
    let mut base_message = config
        .base_message
        .clone()
        .unwrap_or_else(|| String::from("all changes in files"));

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" | "-r" => {
                if let Some(value) = args.next() {
                    roots.push(expand_tilde(&value));
                }
            }
            "--message" | "-m" => {
                if let Some(value) = args.next() {
                    base_message = value;
                }
            }
            _ => roots.push(expand_tilde(&arg)),
        }
    }

    if roots.is_empty() {
        let default_root = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        roots.push(default_root.join("dev"));
    }

    Ok((roots, base_message, config))
}

fn expand_tilde(value: &str) -> PathBuf {
    if let Some(stripped) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(value)
}

fn scan_repos(roots: &[PathBuf]) -> io::Result<Vec<Repo>> {
    let mut seen = HashSet::new();
    let mut repos = Vec::new();

    for root in roots {
        for entry in WalkDir::new(root).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };

            if entry.file_type().is_dir() && entry.file_name() == ".git" {
                if let Some(repo_path) = entry.path().parent() {
                    if let Ok(canonical) = repo_path.canonicalize() {
                        if seen.insert(canonical.clone()) {
                            if let Ok(repo) = read_repo(&canonical) {
                                repos.push(repo);
                            }
                        }
                    }
                }
            }
        }
    }

    repos.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(repos)
}

fn read_repo(path: &Path) -> io::Result<Repo> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain=2", "-b"])
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut branch = String::from("?");
    let mut ahead = 0;
    let mut behind = 0;
    let mut staged = 0;
    let mut unstaged = 0;
    let mut untracked = 0;

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            branch = rest.trim().to_string();
            if branch == "(detached)" {
                branch = String::from("detached");
            }
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let parts: Vec<&str> = rest.split_whitespace().collect();
            for part in parts {
                if let Some(value) = part.strip_prefix('+') {
                    ahead = value.parse().unwrap_or(0);
                } else if let Some(value) = part.strip_prefix('-') {
                    behind = value.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            let mut parts = line.split_whitespace();
            parts.next();
            if let Some(xy) = parts.next() {
                let chars: Vec<char> = xy.chars().collect();
                if chars.len() >= 2 {
                    if chars[0] != '.' {
                        staged += 1;
                    }
                    if chars[1] != '.' {
                        unstaged += 1;
                    }
                }
            }
        } else if line.starts_with("u ") {
            staged += 1;
            unstaged += 1;
        } else if line.starts_with("? ") {
            untracked += 1;
        }
    }

    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();

    let selected = staged + unstaged + untracked > 0;

    Ok(Repo {
        path: path.to_path_buf(),
        name,
        branch,
        ahead,
        behind,
        staged,
        unstaged,
        untracked,
        selected,
        comment: String::new(),
        gen_state: GenState::Idle,
    })
}

fn has_changes(path: &Path) -> io::Result<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(["status", "--porcelain"])
        .output()?;

    if !output.status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    Ok(!output.stdout.is_empty())
}

fn run_git<I, S>(path: &Path, args: I) -> io::Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

fn generate_commit_message(path: &Path, config: &GenConfig) -> io::Result<String> {
    let summary = repo_summary(path)?;
    let prompt = format!(
        "Write a concise git commit message <= 50 chars. Imperative, no quotes.\n{}",
        summary
    );

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(config.timeout_secs))
        .timeout_read(Duration::from_secs(config.timeout_secs))
        .timeout_write(Duration::from_secs(config.timeout_secs))
        .build();

    let response = agent
        .post("https://api.openai.com/v1/chat/completions")
        .set("Authorization", &format!("Bearer {}", config.api_key))
        .set("Content-Type", "application/json")
        .send_json(json!({
            "model": config.model,
            "messages": [{ "role": "user", "content": prompt }],
            "max_tokens": config.max_tokens,
            "temperature": 0.2
        }));

    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("OpenAI {}: {}", code, body.trim()),
            ));
        }
        Err(err) => {
            return Err(io::Error::new(io::ErrorKind::Other, err.to_string()));
        }
    };

    let payload: serde_json::Value = response.into_json().map_err(|err| {
        io::Error::new(io::ErrorKind::Other, format!("Parse error: {}", err))
    })?;

    let content = payload["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("")
        .trim();

    if content.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "Empty response from OpenAI",
        ));
    }

    Ok(sanitize_message(content))
}

fn repo_summary(path: &Path) -> io::Result<String> {
    let status = git_output(path, ["status", "--short"])?;
    let diffstat = git_output(path, ["diff", "--stat"])?;

    let status = limit_lines(&status, 12);
    let diffstat = limit_lines(&diffstat, 8);

    let mut summary = String::new();
    summary.push_str("Status:\n");
    summary.push_str(&truncate_for_prompt(status.trim(), 600));
    summary.push_str("\nDiffstat:\n");
    summary.push_str(&truncate_for_prompt(diffstat.trim(), 600));
    Ok(summary)
}

fn git_output<I, S>(path: &Path, args: I) -> io::Result<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let output = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }
}

fn limit_lines(value: &str, max_lines: usize) -> String {
    value
        .lines()
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_for_prompt(value: &str, max_len: usize) -> String {
    if value.len() <= max_len {
        value.to_string()
    } else {
        let mut truncated = value.chars().take(max_len - 3).collect::<String>();
        truncated.push_str("...");
        truncated
    }
}

fn sanitize_message(value: &str) -> String {
    let single_line = value
        .lines()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();
    truncate_for_prompt(&single_line, 50)
}

fn default_theme() -> Theme {
    Theme {
        background: Color::Rgb(240, 232, 233), // Cloud Dancer
        foreground: Color::Rgb(39, 47, 56),    // Carbon
        header_bg: Color::Rgb(31, 93, 160),    // Strong Blue
        header_fg: Color::Rgb(240, 232, 233),  // Cloud Dancer
        highlight_bg: Color::Rgb(122, 182, 217), // Baltic Sea
        highlight_fg: Color::Rgb(240, 232, 233), // Cloud Dancer
        dirty_fg: Color::Rgb(31, 93, 160),     // Strong Blue
        clean_fg: Color::Rgb(92, 198, 195),    // Rinsing Rivulet
        status_fg: Color::Rgb(39, 47, 56),     // Carbon
        error_fg: Color::Rgb(31, 93, 160),     // Strong Blue
        border_fg: Color::Rgb(197, 192, 208),  // Lavender Blue
    }
}

fn load_theme(theme_path: Option<PathBuf>) -> io::Result<Theme> {
    let env_path = std::env::var("GIT_CONFIRMER_THEME").ok();
    if let Some(path) = env_path {
        if let Ok(theme) = read_theme(Path::new(&path)) {
            return Ok(theme);
        }
    }

    if let Some(path) = theme_path {
        if let Ok(theme) = read_theme(&path) {
            return Ok(theme);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("theme.conf");
        if candidate.exists() {
            return read_theme(&candidate);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("theme.conf");
            if candidate.exists() {
                return read_theme(&candidate);
            }
        }
    }

    Ok(default_theme())
}

fn load_config() -> Option<FileConfig> {
    let env_path = std::env::var("GIT_CONFIRMER_CONFIG").ok();
    if let Some(path) = env_path {
        if let Ok(config) = read_config(Path::new(&path)) {
            return Some(config);
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join("config.toml");
        if candidate.exists() {
            if let Ok(config) = read_config(&candidate) {
                return Some(config);
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join("config.toml");
            if candidate.exists() {
                if let Ok(config) = read_config(&candidate) {
                    return Some(config);
                }
            }
        }
    }

    None
}

fn read_config(path: &Path) -> io::Result<FileConfig> {
    let contents = std::fs::read_to_string(path)?;
    let config: FileConfig = toml::from_str(&contents).map_err(|err| {
        io::Error::new(
            io::ErrorKind::Other,
            format!("config parse error: {}", err),
        )
    })?;
    Ok(config)
}

fn read_theme(path: &Path) -> io::Result<Theme> {
    let contents = std::fs::read_to_string(path)?;
    let mut theme = default_theme();

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or("").trim();
        let value = parts.next().unwrap_or("").trim();
        if value.is_empty() {
            continue;
        }

        if let Some(color) = parse_hex_color(value) {
            match key {
                "background" => theme.background = color,
                "foreground" => theme.foreground = color,
                "header_bg" => theme.header_bg = color,
                "header_fg" => theme.header_fg = color,
                "highlight_bg" => theme.highlight_bg = color,
                "highlight_fg" => theme.highlight_fg = color,
                "dirty_fg" => theme.dirty_fg = color,
                "clean_fg" => theme.clean_fg = color,
                "status_fg" => theme.status_fg = color,
                "error_fg" => theme.error_fg = color,
                "border_fg" => theme.border_fg = color,
                _ => {}
            }
        }
    }

    Ok(theme)
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let value = value.trim();
    let value = value.strip_prefix('#').unwrap_or(value);
    if value.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&value[0..2], 16).ok()?;
    let g = u8::from_str_radix(&value[2..4], 16).ok()?;
    let b = u8::from_str_radix(&value[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}
