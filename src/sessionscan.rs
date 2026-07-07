//! Lightweight session discovery for the `skillscope .` picker.
//!
//! The picker must list EVERY session whose cwd matches the launch
//! directory — including sessions with zero skill invocations, which are
//! invisible to the `SkillInvocation`-based pipeline. So this scan never
//! parses a full transcript: inclusion comes from the dash-encoded project
//! directory name (fast path) or a peek at the first few lines' `cwd`
//! (fallback), and metadata comes cheapest-first from the sessions index,
//! then the peek, then file mtime.

use crate::sessions::{SessionIndex, session_label, session_modified};
use chrono::{DateTime, Utc};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// How many leading lines to peek at when hunting for a `cwd` field.
const PEEK_LINES: usize = 5;

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub session_id: String,
    pub project_path: String,
    pub transcript_path: PathBuf,
    /// Recency signal: index `modified` when present, else file mtime.
    pub last_turn: DateTime<Utc>,
    pub label: String,
    pub git_branch: Option<String>,
}

/// Forward-encode a cwd into its Claude Code project directory name:
/// `/` and `.` both become `-`. The reverse decode is lossy; the forward
/// encode is exact, which is why the fast path keys on it.
pub fn encode_cwd_to_dirname(cwd: &str) -> String {
    cwd.replace(['/', '.'], "-")
}

/// Read up to `PEEK_LINES` lines and return the first `cwd` value found.
fn peek_cwd(transcript_path: &Path) -> Option<String> {
    let file = File::open(transcript_path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines().take(PEEK_LINES) {
        let Ok(line) = line else { continue };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            return Some(cwd.to_string());
        }
    }
    None
}

fn file_mtime_utc(path: &Path) -> Option<DateTime<Utc>> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(DateTime::<Utc>::from(modified))
}

fn summarize(
    transcript_path: PathBuf,
    fallback_project_path: &str,
    index: &SessionIndex,
) -> Option<SessionSummary> {
    let session_id = transcript_path.file_stem()?.to_str()?.to_string();
    let entry = index.get(&session_id);
    let project_path = entry
        .and_then(|e| e.project_path.clone())
        .unwrap_or_else(|| fallback_project_path.to_string());
    let last_turn = session_modified(&session_id, index)
        .or_else(|| file_mtime_utc(&transcript_path))
        .unwrap_or_else(|| DateTime::<Utc>::from_timestamp(0, 0).unwrap());
    let label = session_label(&session_id, index);
    let git_branch = entry.and_then(|e| e.git_branch.clone());
    Some(SessionSummary {
        session_id,
        project_path,
        transcript_path,
        last_turn,
        label,
        git_branch,
    })
}

fn jsonl_files_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// All sessions whose cwd matches `cwd`, sorted by recency descending.
///
/// Fast path: the dash-encoded project directory exists → every top-level
/// `.jsonl` in it belongs to this cwd, no file reads needed for inclusion.
/// Fallback: scan every project dir and peek each transcript's `cwd`.
pub fn discover_sessions_for_cwd(
    projects_dir: &Path,
    cwd: &str,
    index: &SessionIndex,
) -> Vec<SessionSummary> {
    let mut out: Vec<SessionSummary> = Vec::new();

    let encoded_dir = projects_dir.join(encode_cwd_to_dirname(cwd));
    if encoded_dir.is_dir() {
        for path in jsonl_files_in(&encoded_dir) {
            if let Some(summary) = summarize(path, cwd, index) {
                out.push(summary);
            }
        }
    } else {
        let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
            return out;
        };
        for entry in project_dirs.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            for path in jsonl_files_in(&dir) {
                if peek_cwd(&path).as_deref() == Some(cwd)
                    && let Some(summary) = summarize(path, cwd, index)
                {
                    out.push(summary);
                }
            }
        }
    }

    out.sort_by(|a, b| b.last_turn.cmp(&a.last_turn));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::load_session_index;
    use std::fs;
    use tempfile::TempDir;

    fn write_transcript(projects_dir: &Path, project: &str, stem: &str, lines: &[&str]) -> PathBuf {
        let dir = projects_dir.join(project);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{stem}.jsonl"));
        fs::write(&path, lines.join("\n")).unwrap();
        path
    }

    #[test]
    fn encode_cwd_replaces_slashes_and_dots_with_dashes() {
        assert_eq!(
            encode_cwd_to_dirname("/Users/test/workspace"),
            "-Users-test-workspace"
        );
        assert_eq!(
            encode_cwd_to_dirname("/Users/test/.claude/skills"),
            "-Users-test--claude-skills"
        );
        assert_eq!(
            encode_cwd_to_dirname("/Users/test/workspace/github.com/acme"),
            "-Users-test-workspace-github-com-acme"
        );
    }

    #[test]
    fn fast_path_includes_zero_invocation_sessions() {
        let tmp = TempDir::new().unwrap();
        let cwd = "/Users/test/repo";
        // A transcript with no skill invocations at all — just a summary line.
        write_transcript(
            tmp.path(),
            &encode_cwd_to_dirname(cwd),
            "aaaaaaaa-0000-0000-0000-000000000000",
            &[r#"{"type":"summary","summary":"nothing happened"}"#],
        );
        let index = SessionIndex::new();
        let sessions = discover_sessions_for_cwd(tmp.path(), cwd, &index);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            "aaaaaaaa-0000-0000-0000-000000000000"
        );
        assert_eq!(sessions[0].project_path, cwd);
    }

    #[test]
    fn fallback_path_peeks_cwd_when_encoded_dir_absent() {
        let tmp = TempDir::new().unwrap();
        let cwd = "/some/other/place";
        // Project dir name does NOT match the encoded cwd, but the
        // transcript's own cwd field does.
        write_transcript(
            tmp.path(),
            "-unrelated-dirname",
            "bbbbbbbb-0000-0000-0000-000000000000",
            &[
                r#"{"type":"user","cwd":"/some/other/place","sessionId":"x","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"hi"}}"#,
            ],
        );
        // A non-matching transcript in another dir must be excluded.
        write_transcript(
            tmp.path(),
            "-another-dirname",
            "cccccccc-0000-0000-0000-000000000000",
            &[
                r#"{"type":"user","cwd":"/not/it","sessionId":"y","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"hi"}}"#,
            ],
        );
        let index = SessionIndex::new();
        let sessions = discover_sessions_for_cwd(tmp.path(), cwd, &index);
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].session_id,
            "bbbbbbbb-0000-0000-0000-000000000000"
        );
    }

    #[test]
    fn index_modified_wins_over_mtime_and_sorts_desc() {
        let tmp = TempDir::new().unwrap();
        let cwd = "/Users/test/repo";
        let project = encode_cwd_to_dirname(cwd);
        write_transcript(
            tmp.path(),
            &project,
            "dddddddd-0000-0000-0000-000000000000",
            &["{}"],
        );
        write_transcript(
            tmp.path(),
            &project,
            "eeeeeeee-0000-0000-0000-000000000000",
            &["{}"],
        );
        // Index says dddd... is far in the future — it must sort first even
        // though both files were just written (near-identical mtimes).
        fs::write(
            tmp.path().join(&project).join("sessions-index.json"),
            r#"{"version":1,"entries":[
                {"sessionId":"dddddddd-0000-0000-0000-000000000000",
                 "summary":"newest by index",
                 "gitBranch":"main",
                 "modified":"2099-01-01T00:00:00Z"}
            ]}"#,
        )
        .unwrap();
        let index = load_session_index(tmp.path());
        let sessions = discover_sessions_for_cwd(tmp.path(), cwd, &index);
        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0].session_id,
            "dddddddd-0000-0000-0000-000000000000"
        );
        assert_eq!(sessions[0].label, "newest by index");
        assert_eq!(sessions[0].git_branch.as_deref(), Some("main"));
        assert_eq!(
            sessions[0].last_turn.to_rfc3339(),
            "2099-01-01T00:00:00+00:00"
        );
        // The unindexed session degrades to mtime + UUID label.
        assert_eq!(sessions[1].label, "eeeeeeee-0000-0000-0000-000000000000");
    }

    #[test]
    fn no_sessions_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let index = SessionIndex::new();
        assert!(discover_sessions_for_cwd(tmp.path(), "/nowhere", &index).is_empty());
    }
}
