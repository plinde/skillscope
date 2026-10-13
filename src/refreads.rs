//! Reads of a skill's bundled files (`references/*.md`, `scripts/*`, …),
//! with how much of each file a tool call took in.
//!
//! The docs warn that Claude may preview nested reference files with
//! `head -100` and so see only part of them. This module records every tool
//! call that names `skills/<name>/<relpath>` (relpath != `SKILL.md`) and
//! classifies its extent from the tool's own fields: a Read's `limit` /
//! `offset`, or the shell command that names the file (`head -N`,
//! `sed -n 'a,bp'`, `awk 'NR<=N'`, `grep`). It is a separate pass from the
//! invocation parser, so `SkillInvocation` counts never change.

use crate::harness::{file_stem, jsonl_files_under};
use crate::models::{Harness, Origin, ReadExtent, RefRead};
use crate::parser::{
    glob_main_transcripts, glob_subagent_transcripts, is_non_read_tool, load_line, parse_timestamp,
};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// `skills/<name>/<relpath>`. The name class matches `SKILL_PATH_RE`; the
/// relpath class rules out globs and placeholders (checked after the match,
/// since the regex crate has no lookahead).
static SKILL_FILE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"skills/([A-Za-z0-9][A-Za-z0-9_.:-]*)/((?:[A-Za-z0-9_.-]+/)*[A-Za-z0-9_-][A-Za-z0-9_.-]*)",
    )
    .unwrap()
});
/// A Codex code-mode call's shell command: `exec_command({cmd:'…'})`.
static JS_CMD_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r#"cmd\s*:\s*(?:'([^']*)'|"([^"]*)"|`([^`]*)`)"#).unwrap());
static SED_RANGE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+)(?:,(\d+|\$))?p$").unwrap());
static AWK_UPPER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"NR\s*(<=?)\s*(\d+)").unwrap());
static AWK_LOWER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"NR\s*(>=?)\s*(\d+)").unwrap());

/// `(skill, relpath)` for every bundled-file path in `text`, first-seen
/// order, SKILL.md excluded.
pub(crate) fn skill_files_in(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for cap in SKILL_FILE_RE.captures_iter(text) {
        let end = cap.get(0).unwrap().end();
        if text[end..].starts_with(['/', '*', '<', '{', '$']) {
            continue;
        }
        let rel = cap[2].trim_end_matches('.').to_string();
        if rel == "SKILL.md" || rel.is_empty() || rel.split('/').any(|c| c.starts_with('.')) {
            continue;
        }
        let pair = (cap[1].to_string(), rel);
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// Split a shell command at `|`, `||`, `&&`, `;` and newlines outside
/// quotes. Each segment carries whether it reads the previous one's stdout.
fn split_segments(cmd: &str) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut piped = false;
    let (mut single, mut double) = (false, false);
    let chars: Vec<char> = cmd.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            _ => {}
        }
        // `2>&1`, `&>` are redirects, not separators.
        let redirect =
            c == '&' && ((i > 0 && "<>".contains(chars[i - 1])) || chars.get(i + 1) == Some(&'>'));
        if single || double || redirect || !"|&;\n".contains(c) || c == '\'' || c == '"' {
            cur.push(c);
            i += 1;
            continue;
        }
        let next = chars.get(i + 1).copied();
        let (sep_len, next_piped) = match (c, next) {
            ('|', Some('|')) | ('&', Some('&')) => (2, false),
            ('|', _) => (1, true),
            ('&', _) => (1, false),
            _ => (1, false),
        };
        out.push((std::mem::take(&mut cur), piped));
        piped = next_piped;
        i += sep_len;
    }
    out.push((cur, piped));
    out.into_iter()
        .filter(|(s, _)| !s.trim().is_empty())
        .collect()
}

/// Whitespace-split one segment, honouring and stripping quotes.
fn tokenize(segment: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let (mut single, mut double) = (false, false);
    for c in segment.chars() {
        match c {
            '\'' if !double => {
                single = !single;
                in_token = true;
            }
            '"' if !single => {
                double = !double;
                in_token = true;
            }
            c if c.is_whitespace() && !single && !double => {
                if in_token {
                    out.push(std::mem::take(&mut cur));
                    in_token = false;
                }
            }
            c => {
                cur.push(c);
                in_token = true;
            }
        }
    }
    if in_token {
        out.push(cur);
    }
    out
}

/// Line count from `head`/`tail` flags: `-N`, `-n N`, `-nN`, `--lines=N`.
/// Default 10; a byte count or `+K` start offset is an unknown bound.
fn head_tail_lines(args: &[String]) -> ReadExtent {
    let mut lines = Some(10);
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let value = if a == "-n" || a == "--lines" {
            i += 1;
            args.get(i).map(String::as_str)
        } else if let Some(v) = a.strip_prefix("--lines=") {
            Some(v)
        } else if let Some(v) = a.strip_prefix("-n") {
            Some(v.trim_start_matches('='))
        } else if a == "-c" || a.starts_with("-c") || a.starts_with("--bytes") {
            lines = None;
            None
        } else if let Some(v) = a.strip_prefix('-') {
            v.chars().all(|c| c.is_ascii_digit()).then_some(v)
        } else {
            None
        };
        if let Some(v) = value {
            lines = v.parse::<u64>().ok();
        }
        i += 1;
    }
    ReadExtent::Partial { lines }
}

fn sed_extent(args: &[String]) -> Option<ReadExtent> {
    if args
        .iter()
        .any(|a| a == "-i" || (a.starts_with("-i") && !a.starts_with("--")))
    {
        return None; // in-place edit, not a read
    }
    if !args.iter().any(|a| a == "-n" || a == "--quiet") {
        return Some(ReadExtent::Full);
    }
    let mut script = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-e" => {
                script = args.get(i + 1).cloned();
                break;
            }
            a if a.starts_with('-') => {}
            a => {
                script = Some(a.to_string());
                break;
            }
        }
        i += 1;
    }
    let Some(script) = script else {
        return Some(ReadExtent::Unknown);
    };
    let Some(cap) = SED_RANGE_RE.captures(script.trim()) else {
        return Some(ReadExtent::Partial { lines: None });
    };
    let start: u64 = cap[1].parse().unwrap_or(1);
    Some(match cap.get(2).map(|m| m.as_str()) {
        None => ReadExtent::Partial { lines: Some(1) },
        Some("$") if start <= 1 => ReadExtent::Full,
        Some("$") => ReadExtent::Partial { lines: None },
        Some(end) => ReadExtent::Partial {
            lines: end.parse::<u64>().ok().map(|e| e.saturating_sub(start) + 1),
        },
    })
}

fn awk_extent(args: &[String]) -> ReadExtent {
    let Some(prog) = args.iter().find(|a| !a.starts_with('-')) else {
        return ReadExtent::Unknown;
    };
    let Some(upper) = AWK_UPPER_RE.captures(prog) else {
        return ReadExtent::Unknown;
    };
    let mut end: u64 = upper[2].parse().unwrap_or(0);
    if &upper[1] == "<" {
        end = end.saturating_sub(1);
    }
    let start = AWK_LOWER_RE
        .captures(prog)
        .and_then(|c| {
            let n: u64 = c[2].parse().ok()?;
            Some(if &c[1] == ">" { n + 1 } else { n })
        })
        .unwrap_or(1);
    ReadExtent::Partial {
        lines: Some(end.saturating_sub(start) + 1),
    }
}

/// Tools whose input can quote a path without touching the file: plans,
/// todos, questions, scheduling, file-name globs.
const NON_FILE_TOOLS: &[&str] = &[
    "glob",
    "exitplanmode",
    "enterplanmode",
    "todowrite",
    "askuserquestion",
    "taskcreate",
    "taskupdate",
    "workflow",
    "schedulewakeup",
    "croncreate",
    "pushnotification",
    "update_plan",
];

/// Commands that name a file without reading its text.
const NON_READ_COMMANDS: &[&str] = &[
    "ls",
    "wc",
    "stat",
    "test",
    "[",
    "file",
    "cp",
    "mv",
    "rm",
    "trash",
    "mkdir",
    "touch",
    "chmod",
    "git",
    "ln",
    "readlink",
    "realpath",
    "du",
    "find",
    "fd",
    "open",
    "code",
    "echo",
    "printf",
    "cd",
    "shellcheck",
    "gh",
];

/// Extent of one shell segment's read. None = the segment doesn't read
/// file text (`ls`, `git add`, `sed -i`).
fn command_extent(tokens: &[String]) -> Option<ReadExtent> {
    let mut rest = tokens;
    while let Some(first) = rest.first() {
        let is_wrapper = matches!(
            first.as_str(),
            "sudo" | "rtk" | "proxy" | "command" | "env" | "time" | "nice"
        ) || (first.contains('=') && !first.starts_with('-'));
        if !is_wrapper {
            break;
        }
        rest = &rest[1..];
    }
    let (cmd, args) = rest.split_first()?;
    let cmd = cmd.rsplit('/').next().unwrap_or(cmd);
    if NON_READ_COMMANDS.contains(&cmd) {
        return None;
    }
    Some(match cmd {
        "cat" | "bat" | "nl" | "diff" => ReadExtent::Full,
        "head" | "tail" => head_tail_lines(args),
        "sed" | "gsed" => return sed_extent(args),
        "awk" | "gawk" => awk_extent(args),
        "grep" | "egrep" | "fgrep" | "rg" | "ag" | "ack" => ReadExtent::Search,
        _ => ReadExtent::Unknown,
    })
}

/// Bundled-file reads in one shell command. A `cat`/`nl` piped into a
/// limiter (`| head -100`, `| sed -n '1,80p'`, `| grep x`) takes the
/// limiter's extent.
fn shell_reads(cmd: &str) -> Vec<(String, String, ReadExtent)> {
    let segments = split_segments(cmd);
    let mut out: Vec<(String, String, ReadExtent)> = Vec::new();
    for (idx, (segment, _)) in segments.iter().enumerate() {
        let files = skill_files_in(segment);
        if files.is_empty() {
            continue;
        }
        let Some(mut extent) = command_extent(&tokenize(segment)) else {
            continue;
        };
        if extent == ReadExtent::Full
            && let Some((next, true)) = segments.get(idx + 1)
            && let Some(limited) = command_extent(&tokenize(next))
            && limited != ReadExtent::Unknown
        {
            extent = limited;
        }
        for (skill, rel) in files {
            if !out.iter().any(|(s, r, _)| *s == skill && *r == rel) {
                out.push((skill, rel, extent));
            }
        }
    }
    out
}

/// A tool call's input as a JSON object: Codex passes `arguments` as a
/// JSON-encoded string.
fn input_object(input: &Value) -> Option<serde_json::Map<String, Value>> {
    match input {
        Value::Object(m) => Some(m.clone()),
        Value::String(s) => serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| v.as_object().cloned()),
        _ => None,
    }
}

/// The shell command in a tool input: `command` / `cmd` as a string, or an
/// argv array (Codex `shell`: `["bash","-lc","…"]`).
fn command_text(obj: &serde_json::Map<String, Value>) -> Option<String> {
    for key in ["command", "cmd"] {
        match obj.get(key) {
            Some(Value::String(s)) => return Some(s.clone()),
            Some(Value::Array(argv)) => {
                let parts: Vec<&str> = argv.iter().filter_map(|v| v.as_str()).collect();
                if let [shell, flag, script] = parts.as_slice()
                    && shell.ends_with("sh")
                    && flag.starts_with('-')
                {
                    return Some(script.to_string());
                }
                return Some(parts.join(" "));
            }
            _ => {}
        }
    }
    None
}

/// Every bundled-file read in one tool call, with its extent, from the
/// tool's own fields rather than its flattened text.
pub fn read_extents(tool: &str, input: &Value) -> Vec<(String, String, ReadExtent)> {
    if is_non_read_tool(tool) {
        return Vec::new();
    }
    let tool_lc = tool.to_lowercase();
    if NON_FILE_TOOLS.contains(&tool_lc.as_str()) {
        return Vec::new();
    }
    let with = |text: &str, extent: ReadExtent| {
        skill_files_in(text)
            .into_iter()
            .map(|(s, r)| (s, r, extent))
            .collect::<Vec<_>>()
    };
    let Some(obj) = input_object(input) else {
        // Free-form input (Codex code mode): pull out the shell command.
        let text = match input {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        let cmds: Vec<String> = JS_CMD_RE
            .captures_iter(&text)
            .filter_map(|c| c.get(1).or(c.get(2)).or(c.get(3)))
            .map(|m| m.as_str().to_string())
            .collect();
        if cmds.is_empty() {
            return with(&text, ReadExtent::Unknown);
        }
        return cmds.iter().flat_map(|c| shell_reads(c)).collect();
    };
    if tool_lc == "grep" {
        return with(&Value::Object(obj).to_string(), ReadExtent::Search);
    }
    if let Some(path) = obj
        .get("file_path")
        .or_else(|| obj.get("path"))
        .and_then(|v| v.as_str())
        && (tool_lc == "read" || tool_lc == "read_file" || tool_lc == "view")
    {
        let limit = obj.get("limit").and_then(|v| v.as_u64());
        let offset = obj.get("offset").and_then(|v| v.as_u64());
        let extent = match (limit, offset) {
            (Some(n), _) => ReadExtent::Partial { lines: Some(n) },
            (None, Some(o)) if o > 1 => ReadExtent::Partial { lines: None },
            _ => ReadExtent::Full,
        };
        return with(path, extent);
    }
    if let Some(cmd) = command_text(&obj) {
        return shell_reads(&cmd);
    }
    with(&Value::Object(obj).to_string(), ReadExtent::Unknown)
}

struct Ctx<'a> {
    session_id: &'a str,
    timestamp: chrono::DateTime<chrono::Utc>,
    transcript_file: &'a str,
    origin: Origin,
    harness: Harness,
    model: Option<&'a str>,
}

fn push_call(
    out: &mut Vec<RefRead>,
    seen: &mut HashSet<(String, String, String)>,
    key: &str,
    tool: &str,
    input: &Value,
    ctx: &Ctx,
) {
    for (skill_name, rel_path, extent) in read_extents(tool, input) {
        if !seen.insert((key.to_string(), skill_name.clone(), rel_path.clone())) {
            continue;
        }
        out.push(RefRead {
            skill_name,
            rel_path,
            extent,
            tool: tool.to_string(),
            model: ctx.model.map(String::from),
            session_id: ctx.session_id.to_string(),
            timestamp: ctx.timestamp,
            transcript_file: ctx.transcript_file.to_string(),
            origin: ctx.origin,
            harness: ctx.harness,
        });
    }
}

fn extract_claude_file(path: &Path, origin: Origin, out: &mut Vec<RefRead>) {
    let Ok(file) = File::open(path) else { return };
    let transcript_file = path.to_string_lossy().to_string();
    // (message id, skill, relpath): Claude Code writes each content block
    // of one message as its own line.
    let mut seen = HashSet::new();
    for raw_line in BufReader::new(file).lines() {
        let Ok(raw_line) = raw_line else { continue };
        if !raw_line.contains("skills/") {
            continue;
        }
        let Some(data) = load_line(&raw_line) else {
            continue;
        };
        if data.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let (Some(session_id), Some(raw_ts)) = (
            data.get("sessionId").and_then(|v| v.as_str()),
            data.get("timestamp").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let Some(timestamp) = parse_timestamp(raw_ts) else {
            continue;
        };
        let Some(message) = data.get("message") else {
            continue;
        };
        let Some(content) = message.get("content").and_then(|v| v.as_array()) else {
            continue;
        };
        let key = message
            .get("id")
            .or_else(|| data.get("uuid"))
            .and_then(|v| v.as_str())
            .unwrap_or(raw_ts);
        let ctx = Ctx {
            session_id,
            timestamp,
            transcript_file: &transcript_file,
            origin,
            harness: Harness::Claude,
            model: message.get("model").and_then(|v| v.as_str()),
        };
        for entry in content {
            if entry.get("type").and_then(|v| v.as_str()) != Some("tool_use") {
                continue;
            }
            let tool = entry.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if let Some(input) = entry.get("input") {
                push_call(out, &mut seen, key, tool, input, &ctx);
            }
        }
    }
}

/// Bundled-file reads from every Claude Code transcript, main and subagent.
pub fn iter_claude_ref_reads(projects_dir: &Path) -> Vec<RefRead> {
    let mut out = Vec::new();
    for path in glob_main_transcripts(projects_dir) {
        extract_claude_file(&path, Origin::Main, &mut out);
    }
    for path in glob_subagent_transcripts(projects_dir) {
        extract_claude_file(&path, Origin::Subagent, &mut out);
    }
    out
}

fn extract_codex_file(path: &Path, out: &mut Vec<RefRead>) {
    let Ok(file) = File::open(path) else { return };
    let transcript_file = path.to_string_lossy().to_string();
    let mut session_id = file_stem(path);
    let mut origin = Origin::Main;
    let mut model: Option<String> = None;
    let mut seen = HashSet::new();
    for (n, raw_line) in BufReader::new(file).lines().enumerate() {
        let Ok(raw_line) = raw_line else { continue };
        if !raw_line.contains("skills/")
            && !raw_line.contains("\"turn_context\"")
            && !raw_line.contains("\"session_meta\"")
        {
            continue;
        }
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
                if payload
                    .get("source")
                    .is_some_and(|s| s.get("subagent").is_some())
                {
                    origin = Origin::Subagent;
                }
            }
            Some("turn_context") => {
                if let Some(m) = payload.get("model").and_then(|v| v.as_str()) {
                    model = Some(m.to_string());
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
                let tool = payload.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let ctx = Ctx {
                    session_id: &session_id,
                    timestamp,
                    transcript_file: &transcript_file,
                    origin,
                    harness: Harness::Codex,
                    model: model.as_deref(),
                };
                push_call(out, &mut seen, &n.to_string(), tool, input, &ctx);
            }
            _ => {}
        }
    }
}

/// Bundled-file reads from every Codex rollout under `sessions_dir`.
pub fn iter_codex_ref_reads(sessions_dir: &Path) -> Vec<RefRead> {
    let mut out = Vec::new();
    for path in jsonl_files_under(sessions_dir) {
        extract_codex_file(&path, &mut out);
    }
    out
}

fn extract_pi_file(path: &Path, out: &mut Vec<RefRead>) {
    let Ok(file) = File::open(path) else { return };
    let transcript_file = path.to_string_lossy().to_string();
    let mut session_id = file_stem(path);
    let mut seen = HashSet::new();
    for (n, raw_line) in BufReader::new(file).lines().enumerate() {
        let Ok(raw_line) = raw_line else { continue };
        let Some(data) = load_line(&raw_line) else {
            continue;
        };
        match data.get("type").and_then(|v| v.as_str()) {
            Some("session") => {
                if let Some(id) = data.get("id").and_then(|v| v.as_str()) {
                    session_id = id.to_string();
                }
            }
            Some("message") if raw_line.contains("skills/") => {
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
                let ctx = Ctx {
                    session_id: &session_id,
                    timestamp,
                    transcript_file: &transcript_file,
                    origin: Origin::Main,
                    harness: Harness::Pi,
                    model: message.get("model").and_then(|v| v.as_str()),
                };
                for item in content {
                    if item.get("type").and_then(|v| v.as_str()) != Some("toolCall") {
                        continue;
                    }
                    let tool = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    if let Some(args) = item.get("arguments") {
                        push_call(out, &mut seen, &n.to_string(), tool, args, &ctx);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Bundled-file reads from every pi session file under `sessions_dir`.
pub fn iter_pi_ref_reads(sessions_dir: &Path) -> Vec<RefRead> {
    let mut out = Vec::new();
    for path in jsonl_files_under(sessions_dir) {
        extract_pi_file(&path, &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    const REF: &str = "/u/.agents/skills/agent-tuneup/references/phases.md";

    fn one(tool: &str, input: Value) -> Vec<(String, String, ReadExtent)> {
        read_extents(tool, &input)
    }

    fn extent_of(tool: &str, input: Value) -> ReadExtent {
        let got = one(tool, input);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, "agent-tuneup");
        assert_eq!(got[0].1, "references/phases.md");
        got[0].2
    }

    fn bash(cmd: &str) -> ReadExtent {
        extent_of("Bash", json!({ "command": cmd }))
    }

    #[test]
    fn read_tool_limit_decides_full_or_partial() {
        assert_eq!(
            extent_of("Read", json!({"file_path": REF})),
            ReadExtent::Full
        );
        assert_eq!(
            extent_of("Read", json!({"file_path": REF, "limit": 100})),
            ReadExtent::Partial { lines: Some(100) }
        );
        assert_eq!(
            extent_of("Read", json!({"file_path": REF, "offset": 50})),
            ReadExtent::Partial { lines: None }
        );
    }

    #[test]
    fn head_sed_awk_cat_bounds() {
        assert_eq!(
            bash(&format!("head -100 {REF}")),
            ReadExtent::Partial { lines: Some(100) }
        );
        assert_eq!(
            bash(&format!("head -n 40 {REF}")),
            ReadExtent::Partial { lines: Some(40) }
        );
        assert_eq!(
            bash(&format!("head {REF}")),
            ReadExtent::Partial { lines: Some(10) }
        );
        assert_eq!(
            bash(&format!("sed -n '1,80p' {REF}")),
            ReadExtent::Partial { lines: Some(80) }
        );
        assert_eq!(
            bash(&format!("sed -n '21,40p' {REF}")),
            ReadExtent::Partial { lines: Some(20) }
        );
        assert_eq!(bash(&format!("sed -n '1,$p' {REF}")), ReadExtent::Full);
        assert_eq!(
            bash(&format!("awk 'NR<=60' {REF}")),
            ReadExtent::Partial { lines: Some(60) }
        );
        assert_eq!(bash(&format!("cat {REF}")), ReadExtent::Full);
        assert_eq!(bash(&format!("less {REF}")), ReadExtent::Unknown);
    }

    #[test]
    fn search_and_pipes() {
        assert_eq!(bash(&format!("rg foo {REF}")), ReadExtent::Search);
        assert_eq!(
            extent_of("Grep", json!({"pattern": "x", "path": REF})),
            ReadExtent::Search
        );
        assert_eq!(
            bash(&format!("cd /tmp && nl -ba {REF} | sed -n '1,120p'")),
            ReadExtent::Partial { lines: Some(120) }
        );
        assert_eq!(
            bash(&format!("cat {REF} | head -100")),
            ReadExtent::Partial { lines: Some(100) }
        );
        // The segment that names the file decides, not a later unrelated one.
        assert_eq!(
            bash(&format!("cat {REF}; head -5 /etc/hosts")),
            ReadExtent::Full
        );
    }

    #[test]
    fn non_reads_globs_and_skill_md_are_skipped() {
        assert!(one("Bash", json!({"command": format!("ls -la {REF}")})).is_empty());
        assert!(
            one(
                "Bash",
                json!({"command": format!("sed -i '' 's/a/b/' {REF}")})
            )
            .is_empty()
        );
        assert!(one("Edit", json!({"file_path": REF})).is_empty());
        assert!(one("ExitPlanMode", json!({"plan": format!("then cat {REF}")})).is_empty());
        assert!(one("Bash", json!({"command": "cat skills/x/references/*.md"})).is_empty());
        assert!(one("Read", json!({"file_path": "skills/x/SKILL.md"})).is_empty());
    }

    #[test]
    fn codex_argument_shapes() {
        let args = json!({"cmd": format!("sed -n '1,200p' {REF}")}).to_string();
        assert_eq!(
            extent_of("exec_command", Value::String(args)),
            ReadExtent::Partial { lines: Some(200) }
        );
        let shell = json!({"command": ["bash", "-lc", format!("head -100 {REF}")]});
        assert_eq!(
            extent_of("shell", shell),
            ReadExtent::Partial { lines: Some(100) }
        );
        let code = format!("await tools.exec_command({{cmd:'cat {REF}'}})");
        assert_eq!(extent_of("exec", Value::String(code)), ReadExtent::Full);
    }

    fn write(path: &Path, lines: &[String]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = File::create(path).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
    }

    #[test]
    fn claude_transcripts_record_model_and_dedupe_per_message() {
        let tmp = TempDir::new().unwrap();
        let line = |id: &str, input: Value| {
            json!({"type":"assistant","sessionId":"s1","timestamp":"2026-10-01T00:00:00Z",
                "message":{"id":id,"model":"claude-opus-5-5","content":[
                    {"type":"tool_use","name":"Read","input":input}]}})
            .to_string()
        };
        write(
            &tmp.path().join("-repo/s1.jsonl"),
            &[
                line("m1", json!({"file_path": REF, "limit": 100})),
                line("m1", json!({"file_path": REF})),
                line("m2", json!({"file_path": REF})),
                line(
                    "m3",
                    json!({"file_path": "/u/.agents/skills/agent-tuneup/SKILL.md"}),
                ),
            ],
        );
        let reads = iter_claude_ref_reads(tmp.path());
        assert_eq!(reads.len(), 2);
        assert_eq!(reads[0].extent, ReadExtent::Partial { lines: Some(100) });
        assert_eq!(reads[1].extent, ReadExtent::Full);
        assert_eq!(reads[0].model.as_deref(), Some("claude-opus-5-5"));
        // SKILL.md reads stay SkillInvocations only.
        let invs = crate::parser::iter_invocations(tmp.path());
        assert_eq!(invs.len(), 1);
    }

    #[test]
    fn codex_rollouts_take_model_from_turn_context() {
        let tmp = TempDir::new().unwrap();
        let lines = vec![
            r#"{"timestamp":"2026-10-01T00:00:00Z","type":"session_meta","payload":{"id":"cx","source":"cli"}}"#.to_string(),
            r#"{"timestamp":"2026-10-01T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#.to_string(),
            json!({"timestamp":"2026-10-01T00:01:00Z","type":"response_item","payload":{
                "type":"function_call","name":"exec_command",
                "arguments": json!({"cmd": format!("head -100 {REF}")}).to_string()}}).to_string(),
        ];
        write(&tmp.path().join("r.jsonl"), &lines);
        let reads = iter_codex_ref_reads(tmp.path());
        assert_eq!(reads.len(), 1);
        assert_eq!(reads[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(reads[0].session_id, "cx");
        assert_eq!(reads[0].extent, ReadExtent::Partial { lines: Some(100) });
    }
}
