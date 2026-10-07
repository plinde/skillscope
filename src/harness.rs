//! Skill usage recorded by agent harnesses other than Claude Code.
//!
//! - **Codex** (`~/.codex/sessions/**/*.jsonl` rollouts) and **pi**
//!   (`~/.pi/agent/sessions/*/*.jsonl`) have no Skill tool: a skill is used
//!   by reading its `SKILL.md`, so every record is `direct-read`. Only tool
//!   calls are inspected — Codex puts every installed skill's SKILL.md path
//!   in its model-visible skill catalog, so matching whole lines would count
//!   every skill in every session.
//! - **opencode** (`~/.local/share/opencode/opencode.db`) has a native
//!   `skill` tool; its calls are recorded as `claude-proactive`, the
//!   counterpart of Claude Code's Skill tool. The SQLite store is queried
//!   through the `sqlite3` CLI rather than a linked driver.
//!
//! Missing stores yield no records: `--harness all` on a machine without
//! one of these harnesses is not an error.

use crate::models::{Harness, Origin, SkillInvocation, TriggerType};
use crate::parser::{is_non_read_tool, load_line, parse_timestamp, skill_paths_in};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn jsonl_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .map(|e| e.into_path())
        .filter(|p| p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
        .collect();
    out.sort();
    out
}

/// A tool call's input as text: string arguments verbatim, structured ones
/// serialized.
fn input_text(input: &Value) -> String {
    match input {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// One `direct-read` record per distinct skill path in a tool call's input.
#[allow(clippy::too_many_arguments)]
fn push_reads(
    out: &mut Vec<SkillInvocation>,
    tool_name: &str,
    input: &Value,
    session_id: &str,
    project_path: &str,
    timestamp: DateTime<Utc>,
    transcript_file: &str,
    origin: Origin,
    harness: Harness,
) {
    if is_non_read_tool(tool_name) {
        return;
    }
    for skill_name in skill_paths_in(&input_text(input)) {
        out.push(SkillInvocation {
            skill_name,
            trigger_type: TriggerType::DirectRead,
            session_id: session_id.to_string(),
            project_path: project_path.to_string(),
            timestamp,
            transcript_file: transcript_file.to_string(),
            args: None,
            origin,
            harness,
        });
    }
}

pub(crate) fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

fn extract_codex_file(path: &Path, out: &mut Vec<SkillInvocation>) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let transcript_file = path.to_string_lossy().to_string();
    let mut session_id = file_stem(path);
    let mut cwd = String::new();
    let mut origin = Origin::Main;
    for raw_line in BufReader::new(file).lines() {
        let Ok(raw_line) = raw_line else { continue };
        let Some(data) = load_line(&raw_line) else {
            continue;
        };
        let Some(payload) = data.get("payload") else {
            continue;
        };
        match data.get("type").and_then(|v| v.as_str()) {
            Some("session_meta") => {
                if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
                if let Some(dir) = payload.get("cwd").and_then(|v| v.as_str()) {
                    cwd = dir.to_string();
                }
                // `source` is a string ("cli", "vscode", "exec") for a
                // top-level thread and `{"subagent": …}` for a spawned one.
                if payload
                    .get("source")
                    .is_some_and(|s| s.get("subagent").is_some())
                {
                    origin = Origin::Subagent;
                }
            }
            Some("turn_context") => {
                if let Some(dir) = payload.get("cwd").and_then(|v| v.as_str()) {
                    cwd = dir.to_string();
                }
            }
            Some("response_item") => {
                let input = match payload.get("type").and_then(|v| v.as_str()) {
                    Some("function_call") => payload.get("arguments"),
                    Some("custom_tool_call") => payload.get("input"),
                    _ => None,
                };
                let Some(input) = input else { continue };
                let Some(timestamp) = data
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(parse_timestamp)
                else {
                    continue;
                };
                let tool_name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
                push_reads(
                    out,
                    tool_name,
                    input,
                    &session_id,
                    &cwd,
                    timestamp,
                    &transcript_file,
                    origin,
                    Harness::Codex,
                );
            }
            _ => {}
        }
    }
}

/// `direct-read` records from every Codex rollout under `sessions_dir`.
pub fn iter_codex_invocations(sessions_dir: &Path) -> Vec<SkillInvocation> {
    let mut out = Vec::new();
    for path in jsonl_files_under(sessions_dir) {
        extract_codex_file(&path, &mut out);
    }
    out
}

fn extract_pi_file(path: &Path, out: &mut Vec<SkillInvocation>) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let transcript_file = path.to_string_lossy().to_string();
    let mut session_id = file_stem(path);
    let mut cwd = String::new();
    for raw_line in BufReader::new(file).lines() {
        let Ok(raw_line) = raw_line else { continue };
        let Some(data) = load_line(&raw_line) else {
            continue;
        };
        match data.get("type").and_then(|v| v.as_str()) {
            Some("session") => {
                if let Some(id) = data.get("id").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
                if let Some(dir) = data.get("cwd").and_then(|v| v.as_str()) {
                    cwd = dir.to_string();
                }
            }
            Some("message") => {
                let Some(message) = data.get("message") else {
                    continue;
                };
                if message.get("role").and_then(|v| v.as_str()) != Some("assistant") {
                    continue;
                }
                let Some(timestamp) = data
                    .get("timestamp")
                    .and_then(|v| v.as_str())
                    .and_then(parse_timestamp)
                else {
                    continue;
                };
                let Some(content) = message.get("content").and_then(|v| v.as_array()) else {
                    continue;
                };
                // One record per skill per message, however many tool calls
                // in it touch the same file.
                let mut seen: Vec<SkillInvocation> = Vec::new();
                for item in content {
                    if item.get("type").and_then(|v| v.as_str()) != Some("toolCall") {
                        continue;
                    }
                    let Some(arguments) = item.get("arguments") else {
                        continue;
                    };
                    let tool_name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let mut found = Vec::new();
                    push_reads(
                        &mut found,
                        tool_name,
                        arguments,
                        &session_id,
                        &cwd,
                        timestamp,
                        &transcript_file,
                        Origin::Main,
                        Harness::Pi,
                    );
                    for inv in found {
                        if !seen.iter().any(|s| s.skill_name == inv.skill_name) {
                            seen.push(inv);
                        }
                    }
                }
                out.extend(seen);
            }
            _ => {}
        }
    }
}

/// `direct-read` records from every pi session file under `sessions_dir`.
pub fn iter_pi_invocations(sessions_dir: &Path) -> Vec<SkillInvocation> {
    let mut out = Vec::new();
    for path in jsonl_files_under(sessions_dir) {
        extract_pi_file(&path, &mut out);
    }
    out
}

const OPENCODE_SKILL_QUERY: &str = "\
SELECT p.session_id AS session_id, \
       p.time_created AS time_created, \
       json_extract(p.data, '$.state.input.name') AS skill, \
       s.directory AS directory, \
       s.parent_id AS parent_id \
FROM part p JOIN session s ON s.id = p.session_id \
WHERE json_extract(p.data, '$.tool') = 'skill' \
ORDER BY p.time_created";

/// Parse `sqlite3 -json` output of `OPENCODE_SKILL_QUERY` into records.
fn parse_opencode_rows(json: &str, db_path: &str) -> Result<Vec<SkillInvocation>, String> {
    // sqlite3 prints nothing at all (not `[]`) for an empty result set.
    if json.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows: Vec<Value> =
        serde_json::from_str(json).map_err(|e| format!("unreadable sqlite3 output: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let Some(skill_name) = row.get("skill").and_then(|v| v.as_str()) else {
            continue;
        };
        if skill_name.is_empty() {
            continue;
        }
        let Some(timestamp) = row
            .get("time_created")
            .and_then(|v| v.as_i64())
            .and_then(DateTime::<Utc>::from_timestamp_millis)
        else {
            continue;
        };
        let origin = if row.get("parent_id").is_some_and(|v| !v.is_null()) {
            Origin::Subagent
        } else {
            Origin::Main
        };
        out.push(SkillInvocation {
            skill_name: skill_name.to_string(),
            trigger_type: TriggerType::ClaudeProactive,
            session_id: row
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            project_path: row
                .get("directory")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            timestamp,
            transcript_file: db_path.to_string(),
            args: None,
            origin,
            harness: Harness::Opencode,
        });
    }
    Ok(out)
}

/// opencode `skill` tool calls from its SQLite store. A missing database is
/// an empty result; a database that can't be queried (no `sqlite3` on PATH,
/// schema change) is an error the caller reports and skips.
pub fn iter_opencode_invocations(db_path: &Path) -> Result<Vec<SkillInvocation>, String> {
    if !db_path.is_file() {
        return Ok(Vec::new());
    }
    let output = Command::new("sqlite3")
        .arg("-readonly")
        .arg("-json")
        .arg(db_path)
        .arg(OPENCODE_SKILL_QUERY)
        .output()
        .map_err(|e| {
            format!(
                "cannot run sqlite3 ({e}); install it to read {}",
                db_path.display()
            )
        })?;
    if !output.status.success() {
        return Err(format!(
            "sqlite3 failed on {}: {}",
            db_path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_opencode_rows(
        &String::from_utf8_lossy(&output.stdout),
        &db_path.to_string_lossy(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    fn write_lines(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(path).unwrap();
        for line in lines {
            writeln!(f, "{line}").unwrap();
        }
    }

    fn codex_meta(source: &str) -> String {
        format!(
            r#"{{"timestamp":"2026-10-01T00:00:00Z","type":"session_meta","payload":{{"id":"codex-sess","cwd":"/repo","source":{source},"base_instructions":{{"text":"skills: /home/u/.agents/skills/catalog-only/SKILL.md"}}}}}}"#
        )
    }

    fn codex_call(kind: &str, name: &str, field: &str, input: &str) -> String {
        let input = serde_json::to_string(input).unwrap();
        format!(
            r#"{{"timestamp":"2026-10-01T00:01:00Z","type":"response_item","payload":{{"type":"{kind}","name":"{name}","{field}":{input}}}}}"#
        )
    }

    #[test]
    fn codex_tool_calls_reading_skill_md_become_direct_reads() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![
            codex_meta(r#""cli""#),
            codex_call(
                "function_call",
                "exec_command",
                "arguments",
                r#"{"cmd":"sed -n '1,200p' /home/u/.agents/skills/github-cli/SKILL.md; cat /home/u/.agents/skills/worktree/SKILL.md"}"#,
            ),
            codex_call(
                "custom_tool_call",
                "exec",
                "input",
                "await tools.exec_command({cmd:'cat ~/.agents/skills/github-cli/SKILL.md'})",
            ),
            // Authoring a skill is not using it.
            codex_call(
                "custom_tool_call",
                "apply_patch",
                "input",
                "*** Update File: /home/u/.agents/skills/github-cli/SKILL.md",
            ),
            // A user message quoting a path is not a tool call.
            r#"{"timestamp":"2026-10-01T00:02:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"read skills/ignored/SKILL.md"}]}}"#.to_string(),
        ];
        write_lines(&tmp.path().join("2026/10/01/rollout-x.jsonl"), &lines);
        let invs = iter_codex_invocations(tmp.path());
        let names: Vec<&str> = invs.iter().map(|i| i.skill_name.as_str()).collect();
        assert_eq!(names, vec!["github-cli", "worktree", "github-cli"]);
        assert!(
            invs.iter()
                .all(|i| i.trigger_type == TriggerType::DirectRead
                    && i.harness == Harness::Codex
                    && i.origin == Origin::Main
                    && i.session_id == "codex-sess"
                    && i.project_path == "/repo")
        );
    }

    #[test]
    fn codex_subagent_rollouts_get_subagent_origin() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![
            codex_meta(r#"{"subagent":{"other":"guardian"}}"#),
            codex_call(
                "function_call",
                "exec_command",
                "arguments",
                r#"{"cmd":"cat skills/aws/SKILL.md"}"#,
            ),
        ];
        write_lines(&tmp.path().join("rollout-sub.jsonl"), &lines);
        let invs = iter_codex_invocations(tmp.path());
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].origin, Origin::Subagent);
    }

    #[test]
    fn pi_tool_calls_dedupe_per_message() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![
            r#"{"type":"session","id":"pi-sess","timestamp":"2026-10-01T00:00:00Z","cwd":"/work"}"#.to_string(),
            r#"{"type":"message","timestamp":"2026-10-01T00:01:00Z","message":{"role":"assistant","content":[{"type":"toolCall","name":"read","arguments":{"path":"/u/.agents/skills/mfa/SKILL.md"}},{"type":"toolCall","name":"bash","arguments":{"command":"head skills/mfa/SKILL.md"}}]}}"#.to_string(),
            r#"{"type":"message","timestamp":"2026-10-01T00:02:00Z","message":{"role":"assistant","content":[{"type":"toolCall","name":"write","arguments":{"path":"skills/new/SKILL.md"}}]}}"#.to_string(),
            r#"{"type":"message","timestamp":"2026-10-01T00:03:00Z","message":{"role":"user","content":[{"type":"text","text":"see skills/zsh/SKILL.md"}]}}"#.to_string(),
        ];
        write_lines(&tmp.path().join("--work--/s.jsonl"), &lines);
        let invs = iter_pi_invocations(tmp.path());
        assert_eq!(invs.len(), 1);
        assert_eq!(invs[0].skill_name, "mfa");
        assert_eq!(invs[0].harness, Harness::Pi);
        assert_eq!(invs[0].session_id, "pi-sess");
        assert_eq!(invs[0].project_path, "/work");
    }

    #[test]
    fn opencode_rows_parse_into_proactive_records() {
        let json = r#"[{"session_id":"ses_a","time_created":1779976605159,"skill":"kb","directory":"/w","parent_id":null},
            {"session_id":"ses_b","time_created":1779976605200,"skill":"html","directory":"/w","parent_id":"ses_a"},
            {"session_id":"ses_c","time_created":1779976605300,"skill":null,"directory":"/w","parent_id":null}]"#;
        let invs = parse_opencode_rows(json, "/db").unwrap();
        assert_eq!(invs.len(), 2);
        assert_eq!(invs[0].skill_name, "kb");
        assert_eq!(invs[0].trigger_type, TriggerType::ClaudeProactive);
        assert_eq!(invs[0].harness, Harness::Opencode);
        assert_eq!(invs[0].origin, Origin::Main);
        assert_eq!(invs[1].origin, Origin::Subagent);
        assert!(parse_opencode_rows("", "/db").unwrap().is_empty());
    }

    #[test]
    fn missing_stores_yield_nothing() {
        let tmp = TempDir::new().unwrap();
        let absent = tmp.path().join("absent");
        assert!(iter_codex_invocations(&absent).is_empty());
        assert!(iter_pi_invocations(&absent).is_empty());
        assert!(iter_opencode_invocations(&absent).unwrap().is_empty());
    }
}
