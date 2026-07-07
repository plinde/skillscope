//! `skillscope report` — survey skill usage across every session started
//! from one working directory, over an optional time window.
//!
//! Answers the "is my skill readily being called?" question: which skills
//! fired per session, how many times, and in what context — user `/slash`
//! (typed command), claude-proactive (model-invoked via the Skill tool,
//! i.e. keyword/description trigger), and main-vs-subagent origin. Those
//! are the only trigger classes the transcripts record; hooks inject
//! reminders, not skill invocations, so they have no row here.
//!
//! Zero-invocation sessions are counted deliberately: without them there
//! is no denominator for "invoked in N of M sessions".

use crate::models::{Origin, SkillInvocation, TriggerType};
use crate::parser;
use crate::sessions::SessionIndex;
use crate::sessionscan::{self, SessionSummary};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct SkillUsage {
    pub skill_name: String,
    pub total: usize,
    pub user_slash: usize,
    pub claude_proactive: usize,
    pub subagent: usize,
    /// Distinct sessions that invoked this skill at least once.
    pub sessions: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct SessionProfileRow {
    pub session_id: String,
    pub label: String,
    pub git_branch: Option<String>,
    pub last_turn: DateTime<Utc>,
    pub total_invocations: usize,
    pub distinct_skills: usize,
    pub top_skills: Vec<(String, usize)>,
}

#[derive(Debug, Serialize)]
pub struct FocusRow {
    pub session_id: String,
    pub label: String,
    pub count: usize,
    pub user_slash: usize,
    pub claude_proactive: usize,
    pub subagent: usize,
    pub last_ts: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct FocusReport {
    pub skill_name: String,
    pub sessions_invoked: usize,
    pub sessions_total: usize,
    pub rows: Vec<FocusRow>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub cwd: String,
    pub since: Option<DateTime<Utc>>,
    pub sessions_total: usize,
    pub sessions_with_invocations: usize,
    pub skills: Vec<SkillUsage>,
    pub sessions: Vec<SessionProfileRow>,
    pub focus: Option<FocusReport>,
}

/// Build the survey. Sessions are selected by start-cwd and window-filtered
/// on last activity (index `modified`, else file mtime); a selected
/// session's invocations are then counted in full so a long-running session
/// isn't misreported as skill-free.
pub fn build_report(
    projects_dir: &Path,
    cwd: &str,
    since: Option<DateTime<Utc>>,
    focus_skill: Option<&str>,
    index: &SessionIndex,
) -> Report {
    let mut summaries = sessionscan::discover_sessions_for_cwd(projects_dir, cwd, index);
    if let Some(since) = since {
        summaries.retain(|s| s.last_turn >= since);
    }

    let profiles: Vec<(SessionSummary, Vec<SkillInvocation>)> = summaries
        .into_iter()
        .map(|s| {
            let invs = parser::iter_invocations_for_session(&s.transcript_path);
            (s, invs)
        })
        .collect();

    let sessions_total = profiles.len();
    let sessions_with_invocations = profiles.iter().filter(|(_, invs)| !invs.is_empty()).count();

    // Per-skill aggregation across all selected sessions.
    struct Acc {
        total: usize,
        user_slash: usize,
        claude_proactive: usize,
        subagent: usize,
        sessions: std::collections::BTreeSet<String>,
        first_seen: DateTime<Utc>,
        last_seen: DateTime<Utc>,
    }
    let mut per_skill: BTreeMap<String, Acc> = BTreeMap::new();
    for (summary, invs) in &profiles {
        for inv in invs {
            let acc = per_skill
                .entry(inv.skill_name.clone())
                .or_insert_with(|| Acc {
                    total: 0,
                    user_slash: 0,
                    claude_proactive: 0,
                    subagent: 0,
                    sessions: Default::default(),
                    first_seen: inv.timestamp,
                    last_seen: inv.timestamp,
                });
            acc.total += 1;
            match inv.trigger_type {
                TriggerType::UserSlash => acc.user_slash += 1,
                TriggerType::ClaudeProactive => acc.claude_proactive += 1,
            }
            if inv.origin == Origin::Subagent {
                acc.subagent += 1;
            }
            acc.sessions.insert(summary.session_id.clone());
            acc.first_seen = acc.first_seen.min(inv.timestamp);
            acc.last_seen = acc.last_seen.max(inv.timestamp);
        }
    }
    let mut skills: Vec<SkillUsage> = per_skill
        .into_iter()
        .map(|(skill_name, acc)| SkillUsage {
            skill_name,
            total: acc.total,
            user_slash: acc.user_slash,
            claude_proactive: acc.claude_proactive,
            subagent: acc.subagent,
            sessions: acc.sessions.len(),
            first_seen: acc.first_seen,
            last_seen: acc.last_seen,
        })
        .collect();
    skills.sort_by(|a, b| b.total.cmp(&a.total));

    // Per-session profile (input is already sorted by recency descending).
    let sessions: Vec<SessionProfileRow> = profiles
        .iter()
        .map(|(summary, invs)| {
            let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
            for inv in invs {
                *counts.entry(inv.skill_name.as_str()).or_insert(0) += 1;
            }
            let distinct_skills = counts.len();
            let mut top_skills: Vec<(String, usize)> = counts
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            top_skills.sort_by(|a, b| b.1.cmp(&a.1));
            top_skills.truncate(3);
            SessionProfileRow {
                session_id: summary.session_id.clone(),
                label: summary.label.clone(),
                git_branch: summary.git_branch.clone(),
                last_turn: summary.last_turn,
                total_invocations: invs.len(),
                distinct_skills,
                top_skills,
            }
        })
        .collect();

    let focus = focus_skill.map(|skill| {
        let mut rows: Vec<FocusRow> = Vec::new();
        for (summary, invs) in &profiles {
            let hits: Vec<&SkillInvocation> =
                invs.iter().filter(|i| i.skill_name == skill).collect();
            if hits.is_empty() {
                continue;
            }
            rows.push(FocusRow {
                session_id: summary.session_id.clone(),
                label: summary.label.clone(),
                count: hits.len(),
                user_slash: hits
                    .iter()
                    .filter(|i| i.trigger_type == TriggerType::UserSlash)
                    .count(),
                claude_proactive: hits
                    .iter()
                    .filter(|i| i.trigger_type == TriggerType::ClaudeProactive)
                    .count(),
                subagent: hits.iter().filter(|i| i.origin == Origin::Subagent).count(),
                last_ts: hits.iter().map(|i| i.timestamp).max().unwrap(),
            });
        }
        rows.sort_by(|a, b| b.count.cmp(&a.count));
        FocusReport {
            skill_name: skill.to_string(),
            sessions_invoked: rows.len(),
            sessions_total,
            rows,
        }
    });

    Report {
        cwd: cwd.to_string(),
        since,
        sessions_total,
        sessions_with_invocations,
        skills,
        sessions,
        focus,
    }
}
