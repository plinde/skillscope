use clap::Parser;
use skillscope::cli::{Cli, Command};
use skillscope::fzf::{self, FzfOutcome};
use skillscope::{parser, resolve, sessions, sessionscan, tui};
use std::path::Path;

fn main() {
    let cli = Cli::parse();

    if let Some(target) = &cli.target {
        let projects_dir = cli.resolved_projects_dir();
        if target == "." {
            run_picker_mode(&projects_dir);
        } else {
            run_session_mode(&projects_dir, target);
        }
        return;
    }

    match &cli.command {
        None => {
            let invs = parser::iter_invocations(&cli.resolved_projects_dir());
            let index = sessions::load_session_index(&cli.resolved_projects_dir());
            if let Err(e) = tui::run(invs, index) {
                eprintln!("TUI error: {e}");
                std::process::exit(1);
            }
        }
        Some(Command::Summary) => skillscope::cli::cmd_summary(&cli),
        Some(Command::Sessions { skill }) => skillscope::cli::cmd_sessions(&cli, skill),
        Some(Command::Timeline { skill, week }) => {
            skillscope::cli::cmd_timeline(&cli, skill.as_deref(), *week)
        }
        Some(Command::Projects) => skillscope::cli::cmd_projects(&cli),
        Some(Command::Fidelity) => skillscope::cli::cmd_fidelity(&cli),
        Some(Command::Export) => skillscope::cli::cmd_export(&cli),
        Some(Command::Report { skill, cwd }) => {
            skillscope::cli::cmd_report(&cli, skill.as_deref(), cwd.as_deref())
        }
        Some(Command::Inventory { skill, skills_dirs }) => {
            skillscope::cli::cmd_inventory(&cli, skill.as_deref(), skills_dirs)
        }
    }
}

/// `skillscope .` — fzf over every session whose cwd is the launch dir.
fn run_picker_mode(projects_dir: &Path) {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot determine current directory: {e}");
            std::process::exit(1);
        }
    };
    let cwd_str = cwd.to_string_lossy().to_string();
    let index = sessions::load_session_index(projects_dir);
    let summaries = sessionscan::discover_sessions_for_cwd(projects_dir, &cwd_str, &index);
    if summaries.is_empty() {
        eprintln!("No Claude Code sessions found for {cwd_str}.");
        std::process::exit(1);
    }

    let lines = fzf::build_fzf_lines(&summaries);
    let header = format!("Claude Code sessions in {cwd_str}");
    match fzf::run_picker(&lines, "session> ", &header) {
        Ok(FzfOutcome::Selected(session_id)) => run_session_mode(projects_dir, &session_id),
        Ok(FzfOutcome::Cancelled) => {}
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

/// `skillscope <session-id>` — session-scoped TUI, cwd-independent.
fn run_session_mode(projects_dir: &Path, target: &str) {
    let transcript = match resolve::resolve_session_id(projects_dir, target) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let session_id = transcript
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(target)
        .to_string();
    let invs = parser::iter_invocations_for_session(&transcript);
    let index = sessions::load_session_index(projects_dir);
    if let Err(e) = tui::run_scoped(invs, index, session_id) {
        eprintln!("TUI error: {e}");
        std::process::exit(1);
    }
}
