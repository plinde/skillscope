//! Shared data model for skill invocations extracted from Claude Code JSONL
//! transcripts (and, via `harness`, Codex / pi / opencode session stores).
//!
//! This is the contract between the parser, aggregate, fidelity, sessions, and cli/tui layers.
//! Mirrors `skillscope/models.py` in the Python reference, plus an `Origin` field for
//! the Rust rewrite's subagent-transcript feature.

use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TriggerType {
    /// `<command-name>/foo</command-name>` in a `type:"user"` line.
    #[serde(rename = "user-slash")]
    UserSlash,
    /// A Skill tool call whose most recent real user prose named the skill
    /// as `/foo` or `$foo` — the user asked for it in words, the model
    /// carried it out.
    #[serde(rename = "user-named")]
    UserNamed,
    /// `tool_use` with `name:"Skill"` in a `type:"assistant"` line that no
    /// preceding user prose asked for (opencode: its native `skill` tool).
    #[serde(rename = "claude-proactive")]
    ClaudeProactive,
    /// A non-Skill tool call (Read, Bash, Codex `exec_command`, …) whose
    /// input references `skills/<name>/SKILL.md` — the skill was loaded by
    /// reading its file rather than through the Skill tool.
    #[serde(rename = "direct-read")]
    DirectRead,
}

impl TriggerType {
    pub fn label(&self) -> &'static str {
        match self {
            TriggerType::UserSlash => "user-slash",
            TriggerType::UserNamed => "user-named",
            TriggerType::ClaudeProactive => "claude-proactive",
            TriggerType::DirectRead => "direct-read",
        }
    }
}

impl fmt::Display for TriggerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Per-trigger-type counts, shared by every aggregation that breaks a
/// total down by how the skill fired. Flattened into JSON output so each
/// count stays a top-level field.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct TriggerCounts {
    pub user_slash: usize,
    pub user_named: usize,
    pub claude_proactive: usize,
    pub direct_read: usize,
}

impl TriggerCounts {
    pub fn record(&mut self, trigger: TriggerType) {
        match trigger {
            TriggerType::UserSlash => self.user_slash += 1,
            TriggerType::UserNamed => self.user_named += 1,
            TriggerType::ClaudeProactive => self.claude_proactive += 1,
            TriggerType::DirectRead => self.direct_read += 1,
        }
    }
}

/// Which agent harness recorded the invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Harness {
    /// Claude Code — `~/.claude/projects/**/*.jsonl`.
    Claude,
    /// OpenAI Codex CLI — `~/.codex/sessions/**/*.jsonl` rollouts.
    Codex,
    /// pi coding agent — `~/.pi/agent/sessions/*/*.jsonl`.
    Pi,
    /// opencode — `~/.local/share/opencode/opencode.db`.
    Opencode,
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Harness::Claude => "claude",
            Harness::Codex => "codex",
            Harness::Pi => "pi",
            Harness::Opencode => "opencode",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Origin {
    /// Recorded in a top-level `<project>/<session-uuid>.jsonl` transcript.
    #[serde(rename = "main")]
    Main,
    /// Recorded in a `<project>/<session-uuid>/subagents/agent-*.jsonl` transcript.
    #[serde(rename = "subagent")]
    Subagent,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Origin::Main => write!(f, "main"),
            Origin::Subagent => write!(f, "subagent"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillInvocation {
    pub skill_name: String,
    pub trigger_type: TriggerType,
    pub session_id: String,
    /// Decoded `cwd` from the transcript line (or project dir name fallback).
    pub project_path: String,
    pub timestamp: DateTime<Utc>,
    pub transcript_file: String,
    pub args: Option<String>,
    pub origin: Origin,
    pub harness: Harness,
}

/// A skill discovered on disk, for the fidelity layer.
#[derive(Debug, Clone)]
pub struct SkillDefinition {
    pub name: String,
    /// Frontmatter description — the trigger heuristic.
    pub description: String,
    #[allow(dead_code)] // part of the models.py-mirrored contract; not yet surfaced in output
    pub path: String,
    #[allow(dead_code)]
    pub source: String, // user | project | plugin
}

/// A real user prompt from a transcript, for fidelity classification.
#[derive(Debug, Clone)]
pub struct UserPrompt {
    pub text: String,
    pub session_id: String,
    #[allow(dead_code)]
    pub project_path: String,
    #[allow(dead_code)]
    pub timestamp: DateTime<Utc>,
}
