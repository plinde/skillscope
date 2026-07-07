//! Rendering for the three navigation levels: skills table, sessions table,
//! invocations list. Follows the ratatui skill's panel/table-widget pattern.

use super::app::{App, Level};
use crate::models::TriggerType;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Paragraph, Row, Table, TableState};

pub fn draw(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    match app.level {
        Level::Skills => draw_skills(f, app, chunks[0]),
        Level::Sessions => draw_sessions(f, app, chunks[0]),
        Level::Invocations => draw_invocations(f, app, chunks[0]),
        Level::SessionSkills => draw_session_skills(f, app, chunks[0]),
        Level::SessionTimeline => draw_session_timeline(f, app, chunks[0]),
    }

    draw_footer(f, app, chunks[1]);
}

fn block(title: &str) -> Block<'_> {
    Block::default()
        .title(title.to_string())
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
}

fn header_row(cells: &[&str]) -> Row<'static> {
    Row::new(
        cells
            .iter()
            .map(|c| Cell::from(c.to_string()))
            .collect::<Vec<_>>(),
    )
    .style(Style::default().add_modifier(Modifier::BOLD))
}

fn draw_skills(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.skill_rows();
    let title = format!(
        "Skills — sort:{} window:{} ({} skills)",
        app.sort_key.label(),
        app.time_window.label(),
        rows.len()
    );

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(r.name.clone()),
                Cell::from(r.stats.total.to_string()),
                Cell::from(r.stats.user_slash.to_string()),
                Cell::from(r.stats.claude_proactive.to_string()),
                Cell::from(r.stats.subagent.to_string()),
                Cell::from(r.stats.last_seen.date_naive().to_string()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(34),
        Constraint::Percentage(12),
        Constraint::Percentage(12),
        Constraint::Percentage(14),
        Constraint::Percentage(12),
        Constraint::Percentage(16),
    ];

    let table = Table::new(table_rows, widths)
        .header(header_row(&[
            "Skill",
            "Total",
            "Slash",
            "Proactive",
            "Subagent",
            "Last seen",
        ]))
        .block(block(&title))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.selected.min(rows.len() - 1)));
    }
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_sessions(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.session_rows();
    let skill = app.selected_skill.as_deref().unwrap_or("?");
    let title = format!("Sessions invoking '{skill}' ({} sessions)", rows.len());

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            let label = app.session_display_label(&r.session_id);
            let branch = app.session_git_branch(&r.session_id).unwrap_or_default();
            Row::new(vec![
                Cell::from(label),
                Cell::from(branch),
                Cell::from(r.project_path.clone()),
                Cell::from(r.count.to_string()),
                Cell::from(r.subagent_count.to_string()),
                Cell::from(r.last_ts.format("%Y-%m-%d %H:%M").to_string()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(28),
        Constraint::Percentage(14),
        Constraint::Percentage(28),
        Constraint::Percentage(10),
        Constraint::Percentage(10),
        Constraint::Percentage(10),
    ];

    let table = Table::new(table_rows, widths)
        .header(header_row(&[
            "Session", "Branch", "Project", "Count", "Subagent", "Last",
        ]))
        .block(block(&title))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.selected.min(rows.len() - 1)));
    }
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_invocations(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.invocation_rows();
    let skill = app.selected_skill.as_deref().unwrap_or("?");
    let title = format!(
        "Invocations of '{skill}' in session ({} invocations)",
        rows.len()
    );

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|inv| {
            let trigger = match inv.trigger_type {
                TriggerType::UserSlash => "user-slash",
                TriggerType::ClaudeProactive => "claude-proactive",
            };
            Row::new(vec![
                Cell::from(inv.timestamp.format("%Y-%m-%d %H:%M:%S").to_string()),
                Cell::from(trigger),
                Cell::from(inv.origin.to_string()),
                Cell::from(inv.args.clone().unwrap_or_else(|| "-".to_string())),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(22),
        Constraint::Percentage(18),
        Constraint::Percentage(12),
        Constraint::Percentage(48),
    ];

    let table = Table::new(table_rows, widths)
        .header(header_row(&["Timestamp", "Trigger", "Origin", "Args"]))
        .block(block(&title))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.selected.min(rows.len() - 1)));
    }
    f.render_stateful_widget(table, area, &mut state);
}

/// Truncated display label for the scoped session, for panel titles.
fn scoped_session_title(app: &App) -> String {
    let session_id = app.session_scope.as_deref().unwrap_or("?");
    let label = app.session_display_label(session_id);
    label.chars().take(60).collect()
}

fn draw_session_skills(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.session_skill_rows();
    let title = format!(
        "Session '{}' — skills — sort:{} window:{} ({} skills)",
        scoped_session_title(app),
        app.sort_key.label(),
        app.time_window.label(),
        rows.len()
    );

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|r| {
            Row::new(vec![
                Cell::from(r.name.clone()),
                Cell::from(r.stats.total.to_string()),
                Cell::from(r.stats.user_slash.to_string()),
                Cell::from(r.stats.claude_proactive.to_string()),
                Cell::from(r.stats.subagent.to_string()),
                Cell::from(r.stats.last_seen.format("%Y-%m-%d %H:%M").to_string()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(34),
        Constraint::Percentage(12),
        Constraint::Percentage(12),
        Constraint::Percentage(14),
        Constraint::Percentage(12),
        Constraint::Percentage(16),
    ];

    let table = Table::new(table_rows, widths)
        .header(header_row(&[
            "Skill",
            "Total",
            "Slash",
            "Proactive",
            "Subagent",
            "Last seen",
        ]))
        .block(block(&title))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.selected.min(rows.len() - 1)));
    }
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_session_timeline(f: &mut Frame, app: &App, area: Rect) {
    let rows = app.session_timeline_rows();
    let title = format!(
        "Session '{}' — timeline ({} invocations)",
        scoped_session_title(app),
        rows.len()
    );

    let table_rows: Vec<Row> = rows
        .iter()
        .map(|inv| {
            let trigger = match inv.trigger_type {
                TriggerType::UserSlash => "user-slash",
                TriggerType::ClaudeProactive => "claude-proactive",
            };
            Row::new(vec![
                Cell::from(inv.timestamp.format("%Y-%m-%d %H:%M:%S").to_string()),
                Cell::from(inv.skill_name.clone()),
                Cell::from(trigger),
                Cell::from(inv.origin.to_string()),
                Cell::from(inv.args.clone().unwrap_or_else(|| "-".to_string())),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(20),
        Constraint::Percentage(20),
        Constraint::Percentage(16),
        Constraint::Percentage(10),
        Constraint::Percentage(34),
    ];

    let table = Table::new(table_rows, widths)
        .header(header_row(&[
            "Timestamp",
            "Skill",
            "Trigger",
            "Origin",
            "Args",
        ]))
        .block(block(&title))
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    if !rows.is_empty() {
        state.select(Some(app.selected.min(rows.len() - 1)));
    }
    f.render_stateful_widget(table, area, &mut state);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![
        Span::styled(" j/k ", Style::default().fg(Color::Yellow)),
        Span::raw("move  "),
        Span::styled("enter ", Style::default().fg(Color::Yellow)),
        Span::raw("drill in  "),
        Span::styled("backspace/esc ", Style::default().fg(Color::Yellow)),
        Span::raw("back  "),
        Span::styled("/ ", Style::default().fg(Color::Yellow)),
        Span::raw("filter  "),
        Span::styled("t ", Style::default().fg(Color::Yellow)),
        Span::raw("time window  "),
        Span::styled("s ", Style::default().fg(Color::Yellow)),
        Span::raw("sort  "),
        Span::styled("q ", Style::default().fg(Color::Yellow)),
        Span::raw("quit"),
    ];

    if app.session_scope.is_some() {
        spans.insert(0, Span::raw("skills/timeline  "));
        spans.insert(0, Span::styled(" tab ", Style::default().fg(Color::Yellow)));
    }

    if app.filter_editing {
        spans = vec![
            Span::styled(
                " filter: ",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(app.filter.clone()),
            Span::styled("_", Style::default().add_modifier(Modifier::RAPID_BLINK)),
            Span::raw("  (enter/esc to confirm)"),
        ];
    } else if !app.filter.is_empty() {
        spans.insert(
            0,
            Span::styled(
                format!(" [filter: {}] ", app.filter),
                Style::default().fg(Color::Green),
            ),
        );
    }

    let footer = Paragraph::new(Line::from(spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(footer, area);
}
