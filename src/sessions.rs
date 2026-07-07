//! Feature 3: join against `~/.claude/projects/*/sessions-index.json`.
//!
//! Claude Code maintains a per-project session index with human-friendly
//! `summary`/`firstPrompt`/`gitBranch` fields. The index may be stale (a
//! session ran after the last index write) or absent entirely (older
//! projects, or projects that never got indexed) — every lookup degrades
//! gracefully to the raw session UUID.

use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Deserialize)]
struct SessionsIndexFile {
    #[allow(dead_code)]
    version: Option<u32>,
    entries: Vec<SessionIndexEntry>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SessionIndexEntry {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    #[serde(rename = "firstPrompt")]
    pub first_prompt: Option<String>,
    pub summary: Option<String>,
    #[serde(rename = "gitBranch")]
    pub git_branch: Option<String>,
    #[allow(dead_code)] // part of the sessions-index schema; not yet surfaced in output
    #[serde(rename = "messageCount")]
    pub message_count: Option<u64>,
    #[allow(dead_code)]
    #[serde(rename = "isSidechain")]
    pub is_sidechain: Option<bool>,
    /// Original cwd of the session (not the dash-encoded project dir name).
    #[serde(rename = "projectPath")]
    pub project_path: Option<String>,
    /// RFC 3339 last-modified timestamp — the recency signal for the picker.
    pub modified: Option<String>,
}

/// sessionId -> index entry, merged across every project's `sessions-index.json`.
pub type SessionIndex = HashMap<String, SessionIndexEntry>;

/// Load and merge every `sessions-index.json` under `projects_dir`. Missing
/// or unparsable index files are skipped silently (same posture as the
/// transcript parser toward malformed input).
pub fn load_session_index(projects_dir: &Path) -> SessionIndex {
    let mut index = SessionIndex::new();
    let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
        return index;
    };
    for entry in project_dirs.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let index_path = path.join("sessions-index.json");
        let Ok(contents) = std::fs::read_to_string(&index_path) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<SessionsIndexFile>(&contents) else {
            continue;
        };
        for e in parsed.entries {
            index.insert(e.session_id.clone(), e);
        }
    }
    index
}

/// Best-effort display label for a session: summary, else truncated
/// firstPrompt, else the raw UUID.
pub fn session_label(session_id: &str, index: &SessionIndex) -> String {
    if let Some(entry) = index.get(session_id) {
        if let Some(summary) = entry.summary.as_ref().filter(|s| !s.is_empty()) {
            return summary.clone();
        }
        if let Some(first_prompt) = entry.first_prompt.as_ref().filter(|s| !s.is_empty()) {
            let truncated: String = first_prompt.chars().take(80).collect();
            return truncated;
        }
    }
    session_id.to_string()
}

pub fn session_branch(session_id: &str, index: &SessionIndex) -> Option<String> {
    index.get(session_id).and_then(|e| e.git_branch.clone())
}

/// Last-modified time from the index entry, if present and parsable.
pub fn session_modified(session_id: &str, index: &SessionIndex) -> Option<DateTime<Utc>> {
    let raw = index.get(session_id)?.modified.as_ref()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn entry(
        session_id: &str,
        summary: Option<&str>,
        first_prompt: Option<&str>,
        git_branch: Option<&str>,
    ) -> SessionIndexEntry {
        SessionIndexEntry {
            session_id: session_id.to_string(),
            first_prompt: first_prompt.map(String::from),
            summary: summary.map(String::from),
            git_branch: git_branch.map(String::from),
            message_count: None,
            is_sidechain: None,
            project_path: None,
            modified: None,
        }
    }

    fn index_of(entries: Vec<SessionIndexEntry>) -> SessionIndex {
        entries
            .into_iter()
            .map(|e| (e.session_id.clone(), e))
            .collect()
    }

    #[test]
    fn label_prefers_summary_over_first_prompt_and_uuid() {
        let index = index_of(vec![entry(
            "sess-1",
            Some("Fixed the login bug"),
            Some("please fix the login bug"),
            None,
        )]);
        assert_eq!(session_label("sess-1", &index), "Fixed the login bug");
    }

    #[test]
    fn label_falls_back_to_truncated_first_prompt_when_summary_absent() {
        let long_prompt = "x".repeat(200);
        let index = index_of(vec![entry("sess-1", None, Some(&long_prompt), None)]);
        let label = session_label("sess-1", &index);
        assert_eq!(label.chars().count(), 80);
        assert_eq!(label, "x".repeat(80));
    }

    #[test]
    fn label_falls_back_to_truncated_first_prompt_when_summary_is_empty_string() {
        // An empty-string summary must be treated as absent, not as "" itself.
        let index = index_of(vec![entry(
            "sess-1",
            Some(""),
            Some("a real first prompt"),
            None,
        )]);
        assert_eq!(session_label("sess-1", &index), "a real first prompt");
    }

    #[test]
    fn label_falls_back_to_raw_uuid_when_no_index_entry() {
        let index = SessionIndex::new();
        assert_eq!(session_label("sess-unknown", &index), "sess-unknown");
    }

    #[test]
    fn label_falls_back_to_raw_uuid_when_entry_has_no_summary_or_prompt() {
        let index = index_of(vec![entry("sess-1", None, None, None)]);
        assert_eq!(session_label("sess-1", &index), "sess-1");
    }

    #[test]
    fn branch_lookup_returns_none_when_absent_or_unindexed() {
        let index = index_of(vec![entry("sess-1", None, None, None)]);
        assert_eq!(session_branch("sess-1", &index), None);
        assert_eq!(session_branch("sess-unknown", &index), None);

        let index_with_branch = index_of(vec![entry("sess-2", None, None, Some("main"))]);
        assert_eq!(
            session_branch("sess-2", &index_with_branch),
            Some("main".to_string())
        );
    }

    #[test]
    fn load_session_index_merges_across_projects_and_skips_missing_or_malformed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let project_a = tmp.path().join("-project-a");
        let project_b = tmp.path().join("-project-b");
        let project_c_no_index = tmp.path().join("-project-c-no-index");
        fs::create_dir_all(&project_a).unwrap();
        fs::create_dir_all(&project_b).unwrap();
        fs::create_dir_all(&project_c_no_index).unwrap();

        fs::write(
            project_a.join("sessions-index.json"),
            r#"{"version":1,"entries":[{"sessionId":"sess-a","summary":"From project A"}]}"#,
        )
        .unwrap();
        fs::write(
            project_b.join("sessions-index.json"),
            "not valid json at all",
        )
        .unwrap();
        // project_c_no_index intentionally has no sessions-index.json file.

        let index = load_session_index(tmp.path());
        assert_eq!(index.len(), 1);
        assert_eq!(session_label("sess-a", &index), "From project A");
    }

    #[test]
    fn entry_deserializes_project_path_and_modified() {
        let raw = r#"{"sessionId":"sess-1","summary":"s","projectPath":"/Users/test/repo","modified":"2026-01-09T21:19:36.795Z"}"#;
        let e: SessionIndexEntry = serde_json::from_str(raw).unwrap();
        assert_eq!(e.project_path.as_deref(), Some("/Users/test/repo"));
        assert_eq!(e.modified.as_deref(), Some("2026-01-09T21:19:36.795Z"));
    }

    #[test]
    fn session_modified_parses_rfc3339_and_degrades_to_none() {
        let mut with_modified = entry("sess-1", None, None, None);
        with_modified.modified = Some("2026-01-09T21:19:36.795Z".to_string());
        let index = index_of(vec![with_modified]);
        let ts = session_modified("sess-1", &index).expect("parses");
        assert_eq!(ts.to_rfc3339(), "2026-01-09T21:19:36.795+00:00");

        // Absent entry, absent field, and garbage all yield None.
        assert!(session_modified("sess-unknown", &index).is_none());
        let index_no_field = index_of(vec![entry("sess-2", None, None, None)]);
        assert!(session_modified("sess-2", &index_no_field).is_none());
        let mut garbage = entry("sess-3", None, None, None);
        garbage.modified = Some("not a timestamp".to_string());
        let index_garbage = index_of(vec![garbage]);
        assert!(session_modified("sess-3", &index_garbage).is_none());
    }
}
