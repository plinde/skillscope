//! fzf picker wrapper for `skillscope .`.
//!
//! Pure line build/parse functions are separated from the process I/O so
//! they're unit-testable without an fzf binary. Display flags follow the
//! prior art in `~/bin/claude-code-session-search`: the selectable text is
//! tab-delimited with the session id as a trailing hidden field
//! (`--tabstop=1000` pushes it off-screen).

use crate::sessionscan::SessionSummary;
use std::io::Write;
use std::process::{Command, Stdio};

pub enum FzfOutcome {
    Selected(String),
    Cancelled,
}

#[derive(Debug)]
pub enum FzfError {
    NotInstalled,
    Io(std::io::Error),
}

impl std::fmt::Display for FzfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FzfError::NotInstalled => write!(
                f,
                "fzf not found on PATH — install it first (brew install fzf)"
            ),
            FzfError::Io(e) => write!(f, "fzf failed: {e}"),
        }
    }
}

/// One display line per session: visible columns, then a trailing
/// tab-separated session id that `--tabstop=1000` hides off-screen.
pub fn build_fzf_lines(sessions: &[SessionSummary]) -> Vec<String> {
    sessions
        .iter()
        .map(|s| {
            let when = s.last_turn.format("%Y-%m-%d %H:%M");
            let branch = s.git_branch.as_deref().unwrap_or("-");
            let label: String = s.label.chars().take(80).collect();
            format!("{when}  [{branch}]  {label}\t{}", s.session_id)
        })
        .collect()
}

/// Recover the session id from a selected line: the field after the last tab.
pub fn parse_fzf_selection(line: &str) -> Option<String> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let (_, id) = trimmed.rsplit_once('\t')?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Run fzf over `lines`; returns the selected session id or Cancelled.
pub fn run_picker(lines: &[String], prompt: &str, header: &str) -> Result<FzfOutcome, FzfError> {
    let mut child = match Command::new("fzf")
        .args([
            "--height=80%",
            "--layout=reverse",
            "--border=rounded",
            "--tabstop=1000",
            "--no-hscroll",
            "--ansi",
            "--preview-window=hidden",
        ])
        .arg(format!("--prompt={prompt}"))
        .arg(format!("--header={header}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(FzfError::NotInstalled);
        }
        Err(e) => return Err(FzfError::Io(e)),
    };

    {
        let stdin = child.stdin.as_mut().expect("piped stdin");
        for line in lines {
            // fzf exiting early (user already picked/cancelled) closes the
            // pipe; a write error here is not a failure.
            if writeln!(stdin, "{line}").is_err() {
                break;
            }
        }
    }

    let output = child.wait_with_output().map_err(FzfError::Io)?;
    if !output.status.success() {
        // fzf exits 1 on no-match and 130 on ctrl-c/esc — both are a cancel.
        return Ok(FzfOutcome::Cancelled);
    }
    let selection = String::from_utf8_lossy(&output.stdout);
    match parse_fzf_selection(selection.trim_end_matches('\n')) {
        Some(id) => Ok(FzfOutcome::Selected(id)),
        None => Ok(FzfOutcome::Cancelled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use std::path::PathBuf;

    fn summary(id: &str, label: &str, branch: Option<&str>) -> SessionSummary {
        SessionSummary {
            session_id: id.to_string(),
            project_path: "/repo".to_string(),
            transcript_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            last_turn: Utc.with_ymd_and_hms(2026, 7, 1, 12, 30, 0).unwrap(),
            label: label.to_string(),
            git_branch: branch.map(String::from),
        }
    }

    #[test]
    fn lines_end_with_hidden_session_id_field() {
        let sessions = vec![summary("abc12345-0000", "Fixed the bug", Some("main"))];
        let lines = build_fzf_lines(&sessions);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].ends_with("\tabc12345-0000"));
        assert!(lines[0].contains("2026-07-01 12:30"));
        assert!(lines[0].contains("[main]"));
        assert!(lines[0].contains("Fixed the bug"));
    }

    #[test]
    fn missing_branch_renders_as_dash() {
        let sessions = vec![summary("abc12345-0000", "label", None)];
        let lines = build_fzf_lines(&sessions);
        assert!(lines[0].contains("[-]"));
    }

    #[test]
    fn long_labels_are_truncated_to_80_chars() {
        let long = "x".repeat(200);
        let sessions = vec![summary("abc12345-0000", &long, None)];
        let lines = build_fzf_lines(&sessions);
        let visible = lines[0].split('\t').next().unwrap();
        assert!(visible.chars().filter(|c| *c == 'x').count() == 80);
    }

    #[test]
    fn selection_parse_roundtrips_the_built_line() {
        let sessions = vec![summary(
            "abc12345-0000",
            "a label\twith tab-free text",
            None,
        )];
        let lines = build_fzf_lines(&sessions);
        assert_eq!(
            parse_fzf_selection(&lines[0]),
            Some("abc12345-0000".to_string())
        );
    }

    #[test]
    fn selection_parse_handles_trailing_newline_and_empty() {
        assert_eq!(
            parse_fzf_selection("visible\tsess-id\n"),
            Some("sess-id".to_string())
        );
        assert_eq!(parse_fzf_selection(""), None);
        assert_eq!(parse_fzf_selection("visible\t"), None);
    }
}
