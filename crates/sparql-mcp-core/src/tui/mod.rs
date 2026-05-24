//! Terminal project viewer (`sparql-mcp tui`).
//!
//! Read-only: collects stats once at startup and renders a header with global
//! counts plus a scrollable table of projects. Keys: Up/Down or j/k to move,
//! q/Esc to quit.

use std::io::{self, Stdout};
use std::sync::Arc;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::application::stats::{
    collect_project_stats, collect_store_stats, ProjectStat, StoreStats,
};
use crate::domain::SparqlStore;

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

    loop {
        terminal.draw(|f| render(f, &stats, &projects, &mut state))?;

        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => break,
            KeyCode::Down | KeyCode::Char('j') => move_selection(&mut state, &projects, 1),
            KeyCode::Up | KeyCode::Char('k') => move_selection(&mut state, &projects, -1),
            _ => {}
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

fn render(f: &mut Frame, stats: &StoreStats, projects: &[ProjectStat], state: &mut TableState) {
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

    let footer = Paragraph::new(" ↑/↓ or j/k: move   q/Esc: quit")
        .style(Style::default().add_modifier(Modifier::DIM));
    f.render_widget(footer, chunks[2]);
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
        // multibyte: must not panic and must end with the ellipsis
        let long = "é".repeat(60);
        let out = truncate(&long, 10);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 10);
    }

    #[test]
    fn move_selection_wraps_both_ends() {
        let projects = [proj("a"), proj("b"), proj("c")];
        let mut st = TableState::default();
        st.select(Some(0));
        move_selection(&mut st, &projects, -1); // wrap to last
        assert_eq!(st.selected(), Some(2));
        move_selection(&mut st, &projects, 1); // wrap to first
        assert_eq!(st.selected(), Some(0));
    }

    #[test]
    fn move_selection_noop_on_empty() {
        let projects: [ProjectStat; 0] = [];
        let mut st = TableState::default();
        move_selection(&mut st, &projects, 1);
        assert_eq!(st.selected(), None);
    }
}
