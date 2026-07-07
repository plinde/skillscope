//! Shared data model for skill invocations extracted from Claude Code JSONL transcripts.
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
    /// `tool_use` with `name:"Skill"` in a `type:"assistant"` line.
    #[serde(rename = "claude-proactive")]
    ClaudeProactive,
}

impl fmt::Display for TriggerType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TriggerType::UserSlash => write!(f, "user-slash"),
            TriggerType::ClaudeProactive => write!(f, "claude-proactive"),
        }
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
