//! Terminal project viewer (`sparql-mcp tui`).
//!
//! Read-only. The list screen shows global stats + a table of projects. Enter
//! opens a 3-tab detail screen (Detail / Ontologies / Metrics) for the selected
//! project. Keys — list: ↑/↓ or j/k move, Enter open, q/Esc quit; detail:
//! Tab/←/→ or 1·2·3 switch tab, ↑/↓ scroll, Esc/Backspace back.

use std::io::{self, Stdout};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState, Tabs, Wrap};

use crate::application::archive::{self, ArchiveEntry};
use crate::application::detail::{collect_project_detail, ProjectDetail};
use crate::application::stats::{
    collect_project_stats, collect_store_stats, ProjectStat, StoreStats,
};
use crate::domain::SparqlStore;
use crate::xdg;

const TABS: [&str; 4] = ["Detail", "Ontologies", "Metrics", "Backup"];

enum Screen {
    List,
    Detail { tab: usize, scroll: u16 },
    Backups,
}

/// Restores the terminal on drop, even if a panic unwinds through `run`.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        io::stdout().execute(EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = io::stdout().execute(LeaveAlternateScreen);
    }
}

pub fn run(store: Arc<dyn SparqlStore>) -> Result<()> {
    let stats = collect_store_stats(store.as_ref())?;
    let projects = collect_project_stats(store.as_ref())?;

    let _guard = TerminalGuard::enter()?;
    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal: Terminal<CrosstermBackend<Stdout>> = Terminal::new(backend)?;

    let mut state = TableState::default();
    if !projects.is_empty() {
        state.select(Some(0));
    }
    let mut screen = Screen::List;
    let mut detail: Option<ProjectDetail> = None;
    let backups_dir = xdg::backups_dir();
    let mut backups = archive::list_archives(&backups_dir).unwrap_or_default();
    let mut backup_msg: Option<String> = None;
    let mut backups_state = TableState::default();
    if !backups.is_empty() {
        backups_state.select(Some(0));
    }

    loop {
        terminal.draw(|f| match &screen {
            Screen::List => render_list(f, &stats, &projects, &mut state, backup_msg.as_deref()),
            Screen::Detail { tab, scroll } => {
                if let Some(d) = &detail {
                    render_detail(
                        f,
                        d,
                        *tab,
                        *scroll,
                        &backups,
                        &backups_dir,
                        backup_msg.as_deref(),
                    );
                }
            }
            Screen::Backups => render_backups(
                f,
                &backups,
                &backups_dir,
                &mut backups_state,
                backup_msg.as_deref(),
            ),
        })?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match screen {
            Screen::List => match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Down | KeyCode::Char('j') => move_selection(&mut state, &projects, 1),
                KeyCode::Up | KeyCode::Char('k') => move_selection(&mut state, &projects, -1),
                KeyCode::Enter => {
                    if let Some(i) = state.selected() {
                        let p = &projects[i];
                        detail = Some(collect_project_detail(
                            store.as_ref(),
                            &p.id,
                            &p.label,
                            &p.description,
                            &p.graph_iri,
                        )?);
                        screen = Screen::Detail { tab: 0, scroll: 0 };
                    }
                }
                KeyCode::Char('B') => {
                    if backups_state.selected().is_none() && !backups.is_empty() {
                        backups_state.select(Some(0));
                    }
                    screen = Screen::Backups;
                }
                KeyCode::Char('c') => {
                    backup_msg = Some(run_claude(&mut terminal, None));
                }
                _ => {}
            },
            Screen::Detail { tab, scroll } => match key.code {
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => {
                    screen = Screen::List;
                    detail = None;
                }
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') => {
                    screen = Screen::Detail {
                        tab: (tab + 1) % 4,
                        scroll: 0,
                    }
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    screen = Screen::Detail {
                        tab: (tab + 3) % 4,
                        scroll: 0,
                    }
                }
                KeyCode::Char(c @ '1'..='4') => {
                    screen = Screen::Detail {
                        tab: c as usize - '1' as usize,
                        scroll: 0,
                    }
                }
                KeyCode::Enter if tab == 3 => {
                    if backups_state.selected().is_none() && !backups.is_empty() {
                        backups_state.select(Some(0));
                    }
                    screen = Screen::Backups;
                }
                KeyCode::Char('c') => {
                    let prompt = detail.as_ref().map(|d| claude_seed_prompt(&d.id));
                    backup_msg = Some(run_claude(&mut terminal, prompt));
                }
                KeyCode::Char('b') => {
                    let path = archive::default_path(&backups_dir, None);
                    backup_msg = Some(match archive::export_archive(store.as_ref(), &path, None) {
                        Ok(i) => format!("\u{2713} saved latest.zip ({} graphs)", i.graphs),
                        Err(e) => format!("\u{2717} backup failed: {e}"),
                    });
                    backups = archive::list_archives(&backups_dir).unwrap_or_default();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    screen = Screen::Detail {
                        tab,
                        scroll: scroll.saturating_add(1),
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    screen = Screen::Detail {
                        tab,
                        scroll: scroll.saturating_sub(1),
                    }
                }
                _ => {}
            },
            Screen::Backups => match key.code {
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('q') => {
                    screen = Screen::List;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    move_row(&mut backups_state, backups.len(), 1)
                }
                KeyCode::Up | KeyCode::Char('k') => move_row(&mut backups_state, backups.len(), -1),
                KeyCode::F(5) | KeyCode::Char('r') => {
                    let path = archive::default_path(&backups_dir, None);
                    backup_msg = Some(match archive::export_archive(store.as_ref(), &path, None) {
                        Ok(i) => format!("\u{2713} saved latest.zip ({} graphs)", i.graphs),
                        Err(e) => format!("\u{2717} backup failed: {e}"),
                    });
                    backups = archive::list_archives(&backups_dir).unwrap_or_default();
                }
                KeyCode::F(6) | KeyCode::Char('t') => {
                    let tag = archive::next_version_tag(&backups);
                    let path = archive::default_path(&backups_dir, Some(&tag));
                    backup_msg = Some(
                        match archive::export_archive(store.as_ref(), &path, Some(&tag)) {
                            Ok(i) => format!("\u{2713} saved {tag} ({} graphs)", i.graphs),
                            Err(e) => format!("\u{2717} backup failed: {e}"),
                        },
                    );
                    backups = archive::list_archives(&backups_dir).unwrap_or_default();
                }
                KeyCode::Char('c') => {
                    backup_msg = Some(run_claude(&mut terminal, None));
                }
                _ => {}
            },
        }
    }
    Ok(())
}

fn move_selection(state: &mut TableState, projects: &[ProjectStat], delta: isize) {
    if projects.is_empty() {
        return;
    }
    let len = projects.len() as isize;
    let cur = state.selected().unwrap_or(0) as isize;
    let next = (cur + delta).rem_euclid(len);
    state.select(Some(next as usize));
}

fn move_row(state: &mut TableState, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let cur = state.selected().unwrap_or(0) as isize;
    let next = (cur + delta).rem_euclid(len as isize);
    state.select(Some(next as usize));
}

/// Suspend the TUI, run `claude` (optionally seeded with a prompt) attached to
/// the real terminal, then restore the TUI. Returns a status line.
fn run_claude(terminal: &mut Terminal<CrosstermBackend<Stdout>>, prompt: Option<String>) -> String {
    let _ = disable_raw_mode();
    let _ = io::stdout().execute(LeaveAlternateScreen);
    let mut cmd = Command::new("claude");
    if let Some(p) = &prompt {
        cmd.arg(p);
    }
    let result = cmd.status();
    let _ = enable_raw_mode();
    let _ = io::stdout().execute(EnterAlternateScreen);
    let _ = terminal.clear();
    match result {
        Ok(_) => "\u{21A9} returned from claude".to_string(),
        Err(e) => format!("\u{2717} could not launch claude: {e}"),
    }
}

/// Seed prompt that situates claude on a KB project (no filesystem dir exists).
fn claude_seed_prompt(slug: &str) -> String {
    format!(
        "We are working in the sparql-mcp knowledge base on project `{slug}` \
         (named graph <urn:project:{slug}>). The SPARQL store is the source of \
         truth: use the sparql-mcp MCP tools — project_switch to `{slug}`, then \
         query_sparql / update_sparql scoped to <urn:project:{slug}>. \
         What would you like to do on this project?"
    )
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1}M", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

fn render_list(
    f: &mut Frame,
    stats: &StoreStats,
    projects: &[ProjectStat],
    state: &mut TableState,
    status: Option<&str>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled("sparql-mcp", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "  —  {} triples · {} graphs · {} nodes",
            stats.triples, stats.graphs, stats.nodes
        )),
    ]))
    .block(Block::default().borders(Borders::ALL).title(" Store "));
    f.render_widget(header, chunks[0]);

    let rows = projects.iter().map(|p| {
        Row::new(vec![
            Cell::from(p.id.clone()),
            Cell::from(truncate(&p.description, 48)),
            Cell::from(p.triples.to_string()),
            Cell::from(p.nodes.to_string()),
        ])
    });
    let widths = [
        Constraint::Length(22),
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(8),
    ];
    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["project", "description", "triples", "nodes"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(Block::default().borders(Borders::ALL).title(" Projects "))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(table, chunks[1], state);

    let footer_text = match status {
        Some(m) => format!(" {m}"),
        None => " ↑/↓ move   Enter open   B backups   c claude   q quit".to_string(),
    };
    let footer = Paragraph::new(footer_text).style(Style::default().add_modifier(Modifier::DIM));
    f.render_widget(footer, chunks[2]);
}

fn render_backups(
    f: &mut Frame,
    backups: &[ArchiveEntry],
    dir: &Path,
    state: &mut TableState,
    status: Option<&str>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "Backup Manager",
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!("  —  {}", dir.display())),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" KB archives "),
    );
    f.render_widget(header, chunks[0]);

    // newest first
    let mut sorted: Vec<&ArchiveEntry> = backups.iter().collect();
    sorted.sort_by(|a, b| b.created.cmp(&a.created));
    let rows = sorted.iter().map(|e| {
        let file = e
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        Row::new(vec![
            Cell::from(e.created.clone()),
            Cell::from(e.tag.clone().unwrap_or_else(|| "latest".to_string())),
            Cell::from(e.graphs.to_string()),
            Cell::from(human_size(e.bytes)),
            Cell::from(file),
        ])
    });
    let widths = [
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Min(16),
    ];
    let table = Table::new(rows, widths)
        .header(
            Row::new(vec!["date", "tag", "graphs", "size", "file"])
                .style(Style::default().add_modifier(Modifier::BOLD)),
        )
        .block(Block::default().borders(Borders::ALL))
        .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    f.render_stateful_widget(table, chunks[1], state);

    let footer_text = match status {
        Some(m) => format!(" {m}"),
        None => " F5/r latest   F6/t version (vN)   c claude   ↑/↓ move   Esc back".to_string(),
    };
    let footer = Paragraph::new(footer_text).style(Style::default().add_modifier(Modifier::DIM));
    f.render_widget(footer, chunks[2]);
}

#[allow(clippy::too_many_arguments)]
fn render_detail(
    f: &mut Frame,
    d: &ProjectDetail,
    tab: usize,
    scroll: u16,
    backups: &[ArchiveEntry],
    backups_dir: &Path,
    backup_msg: Option<&str>,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    let tabs = Tabs::new(TABS.to_vec())
        .select(tab)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", d.id)),
        )
        .highlight_style(Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED));
    f.render_widget(tabs, chunks[0]);

    let lines = match tab {
        1 => ontologies_lines(d),
        2 => metrics_lines(d),
        3 => backup_lines(backups, backups_dir, backup_msg),
        _ => detail_lines(d),
    };
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
        .block(Block::default().borders(Borders::ALL));
    f.render_widget(body, chunks[1]);

    let footer = Paragraph::new(
        " Tab/←/→ or 1·2·3·4   ↑/↓ scroll   Enter (Backup) → manager   c claude   Esc back",
    )
    .style(Style::default().add_modifier(Modifier::DIM));
    f.render_widget(footer, chunks[2]);
}

fn detail_lines(d: &ProjectDetail) -> Vec<Line<'static>> {
    let mut out = vec![
        Line::from(vec![
            Span::styled(
                "Name        ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(d.label.clone()),
        ]),
        Line::from(vec![
            Span::styled(
                "Project id  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(d.id.clone()),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Description",
            Style::default().add_modifier(Modifier::BOLD),
        )),
    ];
    let desc = if d.description.is_empty() {
        "(none)".to_string()
    } else {
        d.description.clone()
    };
    out.push(Line::from(desc));
    out
}

fn ontologies_lines(d: &ProjectDetail) -> Vec<Line<'static>> {
    if d.classes.is_empty() {
        return vec![Line::from(
            "(no typed classes — ontology not loaded for this graph)",
        )];
    }
    let mut out = Vec::new();
    for c in &d.classes {
        let name = if c.label.is_empty() {
            local_name(&c.iri)
        } else {
            c.label.clone()
        };
        out.push(Line::from(vec![
            Span::styled(
                format!("● {name}"),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("  ({} instances)", c.instances)),
        ]));
        if !c.super_classes.is_empty() {
            let supers: Vec<String> = c.super_classes.iter().map(|s| local_name(s)).collect();
            out.push(Line::from(Span::styled(
                format!("   ⊂ {}", supers.join(", ")),
                Style::default().fg(Color::Cyan),
            )));
        }
        if !c.comment.is_empty() {
            out.push(Line::from(Span::styled(
                format!("   {}", c.comment),
                Style::default().add_modifier(Modifier::DIM),
            )));
        }
        out.push(Line::from(""));
    }
    out
}

fn metrics_lines(d: &ProjectDetail) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    for m in &d.metrics {
        out.push(Line::from(vec![
            Span::styled(
                format!("{:<14}", m.name),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(m.value.clone(), Style::default().fg(Color::Yellow)),
        ]));
        out.push(Line::from(Span::styled(
            format!("   {}", m.explanation),
            Style::default().add_modifier(Modifier::DIM),
        )));
        out.push(Line::from(format!("   → {}", m.interpretation)));
        out.push(Line::from(""));
    }
    out
}

fn backup_lines(backups: &[ArchiveEntry], dir: &Path, msg: Option<&str>) -> Vec<Line<'static>> {
    let latest = dir.join("latest.zip");
    let status = match std::fs::metadata(&latest).and_then(|m| m.modified()) {
        Ok(modified) => match modified.elapsed() {
            Ok(age) => {
                let hours = age.as_secs() / 3600;
                let due = if hours >= 24 { "  ⚠ due (daily)" } else { "" };
                format!("updated {hours}h ago{due}")
            }
            Err(_) => "present".to_string(),
        },
        Err(_) => "none yet".to_string(),
    };
    let mut out = vec![
        Line::from(vec![
            Span::styled(
                "latest.zip  ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(status, Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled(
                "archives    ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("{}", backups.len())),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "A KB archive is a portable container — back up, version, share, re-import.",
            Style::default().add_modifier(Modifier::DIM),
        )),
        Line::from(Span::styled(
            "Press Enter → Backup Manager (dated table · F5 latest · F6 version · c claude).",
            Style::default().fg(Color::Cyan),
        )),
    ];
    if let Some(m) = msg {
        out.push(Line::from(""));
        out.push(Line::from(Span::styled(
            m.to_string(),
            Style::default().fg(Color::Green),
        )));
    }
    out
}

/// Last path/fragment segment of an IRI, for compact display.
fn local_name(iri: &str) -> String {
    iri.rsplit(['#', '/'])
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(iri)
        .to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::stats::ProjectStat;

    fn proj(id: &str) -> ProjectStat {
        ProjectStat {
            id: id.into(),
            label: id.into(),
            description: String::new(),
            graph_iri: format!("urn:project:{id}"),
            triples: 0,
            nodes: 0,
        }
    }

    #[test]
    fn truncate_keeps_short_strings_and_respects_char_boundaries() {
        assert_eq!(truncate("short", 48), "short");
        let long = "é".repeat(60);
        let out = truncate(&long, 10);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 10);
    }

    #[test]
    fn local_name_strips_namespace() {
        assert_eq!(local_name("http://example.org/Dog"), "Dog");
        assert_eq!(local_name("https://sparql-mcp.dev/ns#Project"), "Project");
        // urn: has no #/ separator, so the IRI is returned whole (by design;
        // splitting on ':' would wreck http(s) IRIs).
        assert_eq!(local_name("urn:project:foo"), "urn:project:foo");
    }

    #[test]
    fn move_selection_wraps_both_ends() {
        let projects = [proj("a"), proj("b"), proj("c")];
        let mut st = TableState::default();
        st.select(Some(0));
        move_selection(&mut st, &projects, -1);
        assert_eq!(st.selected(), Some(2));
        move_selection(&mut st, &projects, 1);
        assert_eq!(st.selected(), Some(0));
    }

    #[test]
    fn claude_seed_prompt_names_the_project_graph() {
        let p = claude_seed_prompt("matrix_speedrunner");
        assert!(p.contains("matrix_speedrunner"));
        assert!(p.contains("urn:project:matrix_speedrunner"));
        assert!(p.contains("project_switch"));
    }

    #[test]
    fn human_size_scales() {
        assert_eq!(human_size(512), "512B");
        assert_eq!(human_size(2048), "2.0K");
        assert_eq!(human_size(3 * 1_048_576), "3.0M");
    }

    #[test]
    fn move_row_wraps() {
        let mut st = TableState::default();
        st.select(Some(0));
        move_row(&mut st, 3, -1);
        assert_eq!(st.selected(), Some(2));
        move_row(&mut st, 3, 1);
        assert_eq!(st.selected(), Some(0));
        let mut empty = TableState::default();
        move_row(&mut empty, 0, 1);
        assert_eq!(empty.selected(), None);
    }

    #[test]
    fn move_selection_noop_on_empty() {
        let projects: [ProjectStat; 0] = [];
        let mut st = TableState::default();
        move_selection(&mut st, &projects, 1);
        assert_eq!(st.selected(), None);
    }
}
