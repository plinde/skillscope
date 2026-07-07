//! Feature 4: TUI drill-down (skills → sessions → invocations).
//!
//! Follows the ratatui skill's crossterm-backend / event-loop patterns:
//! raw mode + alternate screen on entry, restored on both clean exit and
//! panic so a crash never leaves the user's terminal wedged.

mod app;
mod ui;

use crate::models::SkillInvocation;
use crate::sessions::SessionIndex;
use app::App;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{self, Stdout};
use std::time::Duration;

pub fn run(invocations: Vec<SkillInvocation>, session_index: SessionIndex) -> io::Result<()> {
    run_app(App::new(invocations, session_index))
}

/// Session-scoped TUI: `invocations` must already be limited to the scoped
/// session's transcripts. Starts at the SessionSkills level; Tab toggles to
/// the flat timeline; esc/q at the top level exits (never falls back to the
/// global skills view).
pub fn run_scoped(
    invocations: Vec<SkillInvocation>,
    session_index: SessionIndex,
    session_id: String,
) -> io::Result<()> {
    run_app(App::new_scoped(invocations, session_index, session_id))
}

fn run_app(mut app: App) -> io::Result<()> {
    let mut terminal = setup_terminal()?;

    // Ensure the terminal is restored even if the render/event loop panics.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        event_loop(&mut terminal, &mut app)
    }));

    restore_terminal(&mut terminal)?;

    match result {
        Ok(res) => res,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            if app.filter_editing {
                match key.code {
                    KeyCode::Esc => app.stop_filter_editing(),
                    KeyCode::Enter => app.stop_filter_editing(),
                    KeyCode::Backspace => app.filter_backspace(),
                    KeyCode::Char(c) => app.filter_push(c),
                    _ => {}
                }
                continue;
            }

            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('/') => app.start_filter_editing(),
                KeyCode::Esc => {
                    if !app.filter.is_empty() {
                        app.clear_filter();
                    } else if app.is_scoped_top_level() {
                        return Ok(());
                    } else {
                        app.go_back();
                    }
                }
                KeyCode::Tab => app.toggle_presentation(),
                KeyCode::Enter => app.drill_in(),
                KeyCode::Backspace => app.go_back(),
                KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                KeyCode::Up | KeyCode::Char('k') => app.select_prev(),
                KeyCode::Char('s') => app.cycle_sort(),
                KeyCode::Char('t') => app.cycle_time_window(),
                _ => {}
            }
        }
    }
}
