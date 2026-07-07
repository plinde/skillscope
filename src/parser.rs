//! Streaming JSONL extraction of skill invocations and user prompts.
//!
//! Reads `~/.claude/projects/*/*.jsonl` transcripts line-by-line so the ~700MB
//! corpus is never fully materialized in memory. Malformed lines are skipped
//! silently — transcripts are append-only logs written by a live process and
//! partial/corrupt trailing lines are expected.
//!
//! Port of `skillscope/parser.py`. Extended with a subagent-transcript walk
//! (`<project>/<session-uuid>/subagents/agent-*.jsonl`) that the Python
//! reference does not cover; those invocations get `Origin::Subagent`.

use crate::models::{Origin, SkillInvocation, TriggerType, UserPrompt};
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// CLI built-ins, not skills — excluded from user-slash extraction.
/// Exact set copied from `parser.py::EXCLUDED_COMMANDS`.
pub const EXCLUDED_COMMANDS: &[&str] = &[
    "clear",
    "model",
    "help",
    "config",
    "compact",
    "exit",
    "login",
    "logout",
    "status",
    "cost",
    "doctor",
    "init",
    "memory",
    "export",
    "resume",
    "tasks",
    "agents",
    "mcp",
    "hooks",
    "permissions",
    "terminal-setup",
    "vim",
    "bug",
    "release-notes",
    "upgrade",
    "usage",
    "todos",
];

fn is_excluded_command(name: &str) -> bool {
    EXCLUDED_COMMANDS.contains(&name.to_lowercase().as_str())
}

static COMMAND_NAME_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<command-name>\s*/?([^<\s]+)\s*</command-name>").unwrap());
static COMMAND_ARGS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"<command-args>([^<]*)</command-args>").unwrap());

fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    // Python: datetime.fromisoformat(raw.replace("Z", "+00:00"))
    let normalized = raw.replace('Z', "+00:00");
    DateTime::parse_from_rfc3339(&normalized)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Best-effort decode of a dashes-encoded project directory name into a path.
///
/// The encoding is lossy (real path components can themselves contain
/// dashes), so this is a fallback only — used when a transcript line has no
/// `cwd` of its own.
fn decode_project_dir(dir_name: &str) -> String {
    let decoded = dir_name.replace('-', "/");
    if decoded.starts_with('/') {
        decoded
    } else {
        format!("/{decoded}")
    }
}

fn load_line(line: &str) -> Option<Value> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(v) if v.is_object() => Some(v),
        _ => None,
    }
}

/// Discover the top-level session transcripts: `<projects_dir>/*/*.jsonl`.
pub(crate) fn glob_main_transcripts(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(project_dirs) = std::fs::read_dir(projects_dir) else {
        return out;
    };
    for entry in project_dirs.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&path) else {
            continue;
        };
        for f in files.flatten() {
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(fp);
            }
        }
    }
    out.sort();
    out
}

/// Discover subagent transcripts: `<projects_dir>/*/<session-uuid>/subagents/agent-*.jsonl`,
/// recursing regardless of how deep the session-uuid directory sits (worktree
/// project directory names can nest further than a single level).
fn glob_subagent_transcripts(projects_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(projects_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_subagent_jsonl = path.extension().and_then(|e| e.to_str()) == Some("jsonl")
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("agent-"))
            && path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("subagents");
        if is_subagent_jsonl {
            out.push(path.to_path_buf());
        }
    }
    out.sort();
    out
}

fn fallback_project_path(jsonl_path: &Path, origin: Origin) -> String {
    // For subagent files the "project dir name" is several levels up:
    // <projects_dir>/<project-dir>/<session-uuid>/subagents/agent-*.jsonl
    let parent = match origin {
        Origin::Main => jsonl_path.parent(),
        Origin::Subagent => jsonl_path
            .parent() // subagents/
            .and_then(|p| p.parent()) // <session-uuid>/
            .and_then(|p| p.parent()), // <project-dir>/
    };
    parent
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(decode_project_dir)
        .unwrap_or_default()
}

fn extract_invocations_from_file(
    jsonl_path: &Path,
    origin: Origin,
    out: &mut Vec<SkillInvocation>,
) {
    let Ok(file) = File::open(jsonl_path) else {
        return;
    };
    let fallback = fallback_project_path(jsonl_path, origin);
    let reader = BufReader::new(file);
    for raw_line in reader.lines() {
        let Ok(raw_line) = raw_line else { continue };
        let Some(data) = load_line(&raw_line) else {
            continue;
        };

        let Some(session_id) = data.get("sessionId").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(raw_ts) = data.get("timestamp").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(timestamp) = parse_timestamp(raw_ts) else {
            continue;
        };

        let Some(message) = data.get("message").filter(|m| m.is_object()) else {
            continue;
        };

        let project_path = data
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| fallback.clone());

        let line_type = data.get("type").and_then(|v| v.as_str());

        match line_type {
            Some("user") => {
                let Some(content) = message.get("content").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(name_match) = COMMAND_NAME_RE.captures(content) else {
                    continue;
                };
                let skill_name = name_match.get(1).unwrap().as_str().trim().to_string();
                if skill_name.is_empty() || is_excluded_command(&skill_name) {
                    continue;
                }
                let args = COMMAND_ARGS_RE.captures(content).and_then(|m| {
                    let text = m.get(1).unwrap().as_str().trim();
                    if text.is_empty() {
                        None
                    } else {
                        Some(text.to_string())
                    }
                });
                out.push(SkillInvocation {
                    skill_name,
                    trigger_type: TriggerType::UserSlash,
                    session_id: session_id.to_string(),
                    project_path,
                    timestamp,
                    transcript_file: jsonl_path.to_string_lossy().to_string(),
                    args,
                    origin,
                });
            }
            Some("assistant") => {
                let Some(content) = message.get("content").and_then(|v| v.as_array()) else {
                    continue;
                };
                for entry in content {
                    let Some(entry) = entry.as_object() else {
                        continue;
                    };
                    if entry.get("type").and_then(|v| v.as_str()) != Some("tool_use")
                        || entry.get("name").and_then(|v| v.as_str()) != Some("Skill")
                    {
                        continue;
                    }
                    let Some(tool_input) = entry.get("input").and_then(|v| v.as_object()) else {
                        continue;
                    };
                    let skill_name = tool_input
                        .get("skill")
                        .and_then(|v| v.as_str())
                        .or_else(|| tool_input.get("command").and_then(|v| v.as_str()));
                    let Some(skill_name) = skill_name else {
                        continue;
                    };
                    if skill_name.is_empty() {
                        continue;
                    }
                    let args = tool_input
                        .get("args")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty())
                        .map(String::from);
                    out.push(SkillInvocation {
                        skill_name: skill_name.to_string(),
                        trigger_type: TriggerType::ClaudeProactive,
                        session_id: session_id.to_string(),
                        project_path: project_path.clone(),
                        timestamp,
                        transcript_file: jsonl_path.to_string_lossy().to_string(),
                        args,
                        origin,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Stream `SkillInvocation` records from every main-session transcript under
/// `projects_dir`, plus every subagent transcript (recursive, any depth).
pub fn iter_invocations(projects_dir: &Path) -> Vec<SkillInvocation> {
    let mut out = Vec::new();
    for path in glob_main_transcripts(projects_dir) {
        extract_invocations_from_file(&path, Origin::Main, &mut out);
    }
    for path in glob_subagent_transcripts(projects_dir) {
        extract_invocations_from_file(&path, Origin::Subagent, &mut out);
    }
    out
}

/// Invocations from one session's main transcript plus any subagent
/// transcripts under the same project directory — the session-scoped view's
/// data source. `main_transcript` is the resolved `<project>/<uuid>.jsonl`;
/// subagent files for other sessions in the same project dir are filtered
/// out downstream by session id.
pub fn iter_invocations_for_session(main_transcript: &Path) -> Vec<SkillInvocation> {
    let mut out = Vec::new();
    extract_invocations_from_file(main_transcript, Origin::Main, &mut out);
    if let Some(project_dir) = main_transcript.parent()
        && let Some(stem) = main_transcript.file_stem()
    {
        // Subagent transcripts live under <project>/<session-uuid>/subagents/.
        let session_dir = project_dir.join(stem);
        if session_dir.is_dir() {
            for path in glob_subagent_transcripts(&session_dir) {
                extract_invocations_from_file(&path, Origin::Subagent, &mut out);
            }
        }
    }
    out
}

/// Same as `iter_invocations` but scoped to main-session transcripts only —
/// used by the parity oracle to match the Python reference, which never
/// walks `subagents/`.
pub fn iter_invocations_main_only(projects_dir: &Path) -> Vec<SkillInvocation> {
    let mut out = Vec::new();
    for path in glob_main_transcripts(projects_dir) {
        extract_invocations_from_file(&path, Origin::Main, &mut out);
    }
    out
}

fn extract_prompt_text(content: &Value) -> Option<String> {
    if let Some(s) = content.as_str() {
        if !s.starts_with('<') && s.len() > 10 {
            return Some(s.to_string());
        }
        return None;
    }
    if let Some(first) = content.as_array().and_then(|arr| arr.first())
        && first.get("type").and_then(|v| v.as_str()) == Some("text")
        && let Some(text) = first.get("text").and_then(|v| v.as_str())
        && !text.starts_with('<')
        && text.len() > 10
    {
        return Some(text.to_string());
    }
    None
}

fn extract_prompts_from_file(jsonl_path: &Path, out: &mut Vec<UserPrompt>) {
    let Ok(file) = File::open(jsonl_path) else {
        return;
    };
    let fallback = fallback_project_path(jsonl_path, Origin::Main);
    let reader = BufReader::new(file);
    for raw_line in reader.lines() {
        let Ok(raw_line) = raw_line else { continue };
        let Some(data) = load_line(&raw_line) else {
            continue;
        };
        if data.get("type").and_then(|v| v.as_str()) != Some("user") {
            continue;
        }
        // Synthetic/meta records are not real user asks: isMeta lines,
        // subagent sidechains (e.g. title-generator prompts), and
        // tool-result carriers all masquerade as type:"user".
        let is_meta = data
            .get("isMeta")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let is_sidechain = data
            .get("isSidechain")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let has_tool_result = !data
            .get("toolUseResult")
            .map(Value::is_null)
            .unwrap_or(true);
        let has_source_tool_uuid = data.get("sourceToolAssistantUUID").is_some();
        if is_meta || is_sidechain || has_tool_result || has_source_tool_uuid {
            continue;
        }
        // promptSource "sdk"/"system" marks harness-generated prompts
        // (e.g. conversation-title generators); "typed"/"queued" are
        // real user input, None predates the field — keep those.
        if let Some(ps) = data.get("promptSource").and_then(|v| v.as_str())
            && (ps == "sdk" || ps == "system")
        {
            continue;
        }

        let Some(session_id) = data.get("sessionId").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(raw_ts) = data.get("timestamp").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(timestamp) = parse_timestamp(raw_ts) else {
            continue;
        };

        let Some(message) = data.get("message").filter(|m| m.is_object()) else {
            continue;
        };
        let Some(content) = message.get("content") else {
            continue;
        };
        let Some(text) = extract_prompt_text(content) else {
            continue;
        };

        let project_path = data
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| fallback.clone());

        let truncated: String = text.chars().take(500).collect();
        out.push(UserPrompt {
            text: truncated,
            session_id: session_id.to_string(),
            project_path,
            timestamp,
        });
    }
}

/// Stream real free-text user prompts (for the fidelity layer to correlate).
/// Main-session transcripts only — matches the Python reference's scope.
pub fn iter_user_prompts(projects_dir: &Path) -> Vec<UserPrompt> {
    let mut out = Vec::new();
    for path in glob_main_transcripts(projects_dir) {
        extract_prompts_from_file(&path, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn user_slash_line(session_id: &str, command: &str, args: Option<&str>) -> String {
        let args_tag = args
            .map(|a| format!("<command-args>{a}</command-args>"))
            .unwrap_or_default();
        format!(
            r#"{{"type":"user","sessionId":"{session_id}","timestamp":"2026-01-01T00:00:00Z","cwd":"/repo","message":{{"role":"user","content":"<command-name>/{command}</command-name>{args_tag}"}}}}"#
        )
    }

    fn claude_proactive_line(session_id: &str, skill: &str) -> String {
        format!(
            r#"{{"type":"assistant","sessionId":"{session_id}","timestamp":"2026-01-02T00:00:00Z","cwd":"/repo","message":{{"role":"assistant","content":[{{"type":"tool_use","name":"Skill","input":{{"skill":"{skill}"}}}}]}}}}"#
        )
    }

    fn write_main_transcript(projects_dir: &Path, project_dir: &str, lines: &[String]) -> PathBuf {
        let dir = projects_dir.join(project_dir);
        fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("session.jsonl");
        let mut f = File::create(&file_path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
        file_path
    }

    // -- load_line / malformed-line handling --------------------------------

    #[test]
    fn load_line_skips_blank_and_malformed() {
        assert!(load_line("").is_none());
        assert!(load_line("   ").is_none());
        assert!(load_line("not json at all").is_none());
        assert!(load_line("{\"truncated\": ").is_none());
        assert!(load_line("[1, 2, 3]").is_none()); // valid JSON, not an object
        assert!(load_line(r#"{"a": 1}"#).is_some());
    }

    #[test]
    fn iter_invocations_skips_malformed_lines_without_failing_the_file() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![
            "not json".to_string(),
            user_slash_line("sess-1", "worktree", None),
            "{\"incomplete\":".to_string(),
        ];
        write_main_transcript(tmp.path(), "-repo-one", &lines);
        let invs = iter_invocations_main_only(tmp.path());
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].skill_name, "worktree");
    }

    // -- is_excluded_command --------------------------------------------------

    #[test]
    fn excluded_commands_are_case_insensitive() {
        assert!(is_excluded_command("clear"));
        assert!(is_excluded_command("CLEAR"));
        assert!(is_excluded_command("Compact"));
        assert!(!is_excluded_command("worktree"));
    }

    #[test]
    fn user_slash_skips_excluded_commands() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![
            user_slash_line("sess-1", "clear", None),
            user_slash_line("sess-1", "worktree", None),
        ];
        write_main_transcript(tmp.path(), "-repo-one", &lines);
        let invs = iter_invocations_main_only(tmp.path());
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].skill_name, "worktree");
    }

    // -- record classification: user-slash vs claude-proactive ---------------

    #[test]
    fn classifies_user_slash_invocation() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![user_slash_line("sess-1", "worktree", Some("foo bar"))];
        write_main_transcript(tmp.path(), "-repo-one", &lines);
        let invs = iter_invocations_main_only(tmp.path());
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].trigger_type, TriggerType::UserSlash);
        assert_eq!(invs[0].skill_name, "worktree");
        assert_eq!(invs[0].args.as_deref(), Some("foo bar"));
        assert_eq!(invs[0].origin, Origin::Main);
    }

    #[test]
    fn classifies_claude_proactive_invocation() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![claude_proactive_line("sess-1", "cve-lookup")];
        write_main_transcript(tmp.path(), "-repo-one", &lines);
        let invs = iter_invocations_main_only(tmp.path());
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].trigger_type, TriggerType::ClaudeProactive);
        assert_eq!(invs[0].skill_name, "cve-lookup");
        assert_eq!(invs[0].origin, Origin::Main);
    }

    #[test]
    fn claude_proactive_ignores_non_skill_tool_use() {
        let tmp = TempDir::new().unwrap();
        let line = r#"{"type":"assistant","sessionId":"sess-1","timestamp":"2026-01-02T00:00:00Z","cwd":"/repo","message":{"role":"assistant","content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#.to_string();
        write_main_transcript(tmp.path(), "-repo-one", &[line]);
        let invs = iter_invocations_main_only(tmp.path());
        assert!(invs.is_empty());
    }

    // -- noise filters (extract_prompts_from_file) ----------------------------

    fn user_prompt_line(extra_fields: &str, text: &str) -> String {
        format!(
            r#"{{"type":"user","sessionId":"sess-1","timestamp":"2026-01-01T00:00:00Z","cwd":"/repo",{extra_fields}"message":{{"role":"user","content":"{text}"}}}}"#
        )
    }

    #[test]
    fn user_prompt_is_meta_is_filtered() {
        let tmp = TempDir::new().unwrap();
        let line = user_prompt_line(
            r#""isMeta":true,"#,
            "this is a long enough real user prompt text",
        );
        write_main_transcript(tmp.path(), "-repo-one", &[line]);
        let mut out = Vec::new();
        for path in glob_main_transcripts(tmp.path()) {
            extract_prompts_from_file(&path, &mut out);
        }
        assert!(out.is_empty());
    }

    #[test]
    fn user_prompt_is_sidechain_is_filtered() {
        let tmp = TempDir::new().unwrap();
        let line = user_prompt_line(
            r#""isSidechain":true,"#,
            "this is a long enough real user prompt text",
        );
        write_main_transcript(tmp.path(), "-repo-one", &[line]);
        let mut out = Vec::new();
        for path in glob_main_transcripts(tmp.path()) {
            extract_prompts_from_file(&path, &mut out);
        }
        assert!(out.is_empty());
    }

    #[test]
    fn user_prompt_with_tool_use_result_is_filtered() {
        let tmp = TempDir::new().unwrap();
        let line = user_prompt_line(
            r#""toolUseResult":{"ok":true},"#,
            "this is a long enough real user prompt text",
        );
        write_main_transcript(tmp.path(), "-repo-one", &[line]);
        let mut out = Vec::new();
        for path in glob_main_transcripts(tmp.path()) {
            extract_prompts_from_file(&path, &mut out);
        }
        assert!(out.is_empty());
    }

    #[test]
    fn user_prompt_with_source_tool_assistant_uuid_is_filtered() {
        let tmp = TempDir::new().unwrap();
        let line = user_prompt_line(
            r#""sourceToolAssistantUUID":"abc-123","#,
            "this is a long enough real user prompt text",
        );
        write_main_transcript(tmp.path(), "-repo-one", &[line]);
        let mut out = Vec::new();
        for path in glob_main_transcripts(tmp.path()) {
            extract_prompts_from_file(&path, &mut out);
        }
        assert!(out.is_empty());
    }

    #[test]
    fn user_prompt_source_sdk_or_system_is_filtered() {
        let tmp = TempDir::new().unwrap();
        let sdk_line = user_prompt_line(
            r#""promptSource":"sdk","#,
            "this is a long enough real user prompt text",
        );
        let system_line = user_prompt_line(
            r#""promptSource":"system","#,
            "this is another long enough real user prompt",
        );
        write_main_transcript(tmp.path(), "-repo-one", &[sdk_line, system_line]);
        let mut out = Vec::new();
        for path in glob_main_transcripts(tmp.path()) {
            extract_prompts_from_file(&path, &mut out);
        }
        assert!(out.is_empty());
    }

    #[test]
    fn user_prompt_source_typed_or_absent_is_kept() {
        let tmp = TempDir::new().unwrap();
        let typed_line = user_prompt_line(
            r#""promptSource":"typed","#,
            "this is a long enough real user prompt text",
        );
        let no_source_line =
            user_prompt_line("", "this is another real user prompt with no source");
        write_main_transcript(tmp.path(), "-repo-one", &[typed_line, no_source_line]);
        let mut out = Vec::new();
        for path in glob_main_transcripts(tmp.path()) {
            extract_prompts_from_file(&path, &mut out);
        }
        assert_eq!(out.len(), 2);
    }

    // -- cwd fallback decoding -------------------------------------------------

    #[test]
    fn decode_project_dir_replaces_dashes_and_ensures_leading_slash() {
        assert_eq!(
            decode_project_dir("-Users-test-workspace"),
            "/Users/test/workspace"
        );
        assert_eq!(decode_project_dir("no-leading-slash"), "/no/leading/slash");
    }

    #[test]
    fn cwd_field_takes_precedence_over_fallback() {
        let tmp = TempDir::new().unwrap();
        let line = r#"{"type":"user","sessionId":"sess-1","timestamp":"2026-01-01T00:00:00Z","cwd":"/explicit/cwd","message":{"role":"user","content":"<command-name>/worktree</command-name>"}}"#.to_string();
        write_main_transcript(tmp.path(), "-fallback-project-dir", &[line]);
        let invs = iter_invocations_main_only(tmp.path());
        assert_eq!(invs[0].project_path, "/explicit/cwd");
    }

    #[test]
    fn missing_cwd_falls_back_to_decoded_project_dir_name() {
        let tmp = TempDir::new().unwrap();
        let line = r#"{"type":"user","sessionId":"sess-1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"<command-name>/worktree</command-name>"}}"#.to_string();
        write_main_transcript(tmp.path(), "-fallback-project-dir", &[line]);
        let invs = iter_invocations_main_only(tmp.path());
        assert_eq!(invs[0].project_path, "/fallback/project/dir");
    }

    // -- subagent origin detection ---------------------------------------------

    #[test]
    fn subagent_transcripts_get_subagent_origin_and_main_stays_main() {
        let tmp = TempDir::new().unwrap();
        // main transcript
        write_main_transcript(
            tmp.path(),
            "-repo-one",
            &[claude_proactive_line("sess-1", "main-skill")],
        );
        // subagent transcript: <projects_dir>/<project>/<session-uuid>/subagents/agent-1.jsonl
        let subagent_dir = tmp
            .path()
            .join("-repo-one")
            .join("sess-1")
            .join("subagents");
        fs::create_dir_all(&subagent_dir).unwrap();
        let mut f = File::create(subagent_dir.join("agent-1.jsonl")).unwrap();
        writeln!(f, "{}", claude_proactive_line("sess-1-sub", "sub-skill")).unwrap();

        let invs = iter_invocations(tmp.path());
        assert_eq!(invs.len(), 2);
        let main_inv = invs.iter().find(|i| i.skill_name == "main-skill").unwrap();
        let sub_inv = invs.iter().find(|i| i.skill_name == "sub-skill").unwrap();
        assert_eq!(main_inv.origin, Origin::Main);
        assert_eq!(sub_inv.origin, Origin::Subagent);

        // iter_invocations_main_only must not see the subagent record at all
        // (matches the Python reference's scope, used by the parity oracle).
        let main_only = iter_invocations_main_only(tmp.path());
        assert_eq!(main_only.len(), 1);
        assert_eq!(main_only[0].skill_name, "main-skill");
    }

    #[test]
    fn subagent_glob_ignores_jsonl_outside_subagents_dir_or_without_agent_prefix() {
        let tmp = TempDir::new().unwrap();
        write_main_transcript(tmp.path(), "-repo-one", &[]);
        // Not inside a "subagents" directory - must not be picked up as subagent.
        let stray_dir = tmp.path().join("-repo-one").join("sess-1").join("notes");
        fs::create_dir_all(&stray_dir).unwrap();
        let mut f = File::create(stray_dir.join("agent-1.jsonl")).unwrap();
        writeln!(
            f,
            "{}",
            claude_proactive_line("sess-1", "should-not-appear")
        )
        .unwrap();
        // Inside subagents/ but wrong filename prefix - must also be skipped.
        let subagent_dir = tmp
            .path()
            .join("-repo-one")
            .join("sess-1")
            .join("subagents");
        fs::create_dir_all(&subagent_dir).unwrap();
        let mut f2 = File::create(subagent_dir.join("other-1.jsonl")).unwrap();
        writeln!(
            f2,
            "{}",
            claude_proactive_line("sess-1", "also-should-not-appear")
        )
        .unwrap();

        assert!(glob_subagent_transcripts(tmp.path()).is_empty());
    }

    // -- timestamp parsing -------------------------------------------------------

    #[test]
    fn parse_timestamp_accepts_zulu_and_rejects_garbage() {
        assert!(parse_timestamp("2026-01-01T00:00:00Z").is_some());
        assert!(parse_timestamp("2026-01-01T00:00:00+00:00").is_some());
        assert!(parse_timestamp("not-a-timestamp").is_none());
    }

    #[test]
    fn missing_or_unparsable_timestamp_drops_the_record() {
        let tmp = TempDir::new().unwrap();
        let line = r#"{"type":"user","sessionId":"sess-1","timestamp":"garbage","cwd":"/repo","message":{"role":"user","content":"<command-name>/worktree</command-name>"}}"#.to_string();
        write_main_transcript(tmp.path(), "-repo-one", &[line]);
        assert!(iter_invocations_main_only(tmp.path()).is_empty());
    }
}
