//! Pure aggregation functions over `SkillInvocation` slices.
//!
//! No I/O here — everything takes a `&[SkillInvocation]` (typically from
//! `parser::iter_invocations`) and returns plain structs. Callers
//! materialize/filter the slice (e.g. by `--since`) before calling in.
//!
//! Port of `skillscope/aggregate.py`.

use crate::models::{Origin, SkillInvocation, TriggerType};
use chrono::{DateTime, Datelike, NaiveDate, Utc};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct SkillCountEntry {
    pub total: usize,
    pub user_slash: usize,
    pub claude_proactive: usize,
    pub subagent: usize,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Per-skill totals, trigger-type breakdown, subagent-origin count, and
/// first/last-seen timestamps. Keyed by skill name; iteration order is not
/// significant (callers sort separately, matching the Python's `dict` +
/// explicit `sorted()` pattern).
pub fn skill_counts(invs: &[SkillInvocation]) -> BTreeMap<String, SkillCountEntry> {
    let mut counts: BTreeMap<String, SkillCountEntry> = BTreeMap::new();
    for inv in invs {
        let entry = counts
            .entry(inv.skill_name.clone())
            .or_insert_with(|| SkillCountEntry {
                total: 0,
                user_slash: 0,
                claude_proactive: 0,
                subagent: 0,
                first_seen: inv.timestamp,
                last_seen: inv.timestamp,
            });
        entry.total += 1;
        match inv.trigger_type {
            TriggerType::UserSlash => entry.user_slash += 1,
            TriggerType::ClaudeProactive => entry.claude_proactive += 1,
        }
        if inv.origin == Origin::Subagent {
            entry.subagent += 1;
        }
        if inv.timestamp < entry.first_seen {
            entry.first_seen = inv.timestamp;
        }
        if inv.timestamp > entry.last_seen {
            entry.last_seen = inv.timestamp;
        }
    }
    counts
}

#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub session_id: String,
    pub project_path: String,
    pub count: usize,
    pub first_ts: DateTime<Utc>,
    pub last_ts: DateTime<Utc>,
    pub subagent_count: usize,
}

/// Sessions that fired `skill`, with per-session count and time span,
/// sorted by count descending (matches Python's `sessions_for_skill`).
pub fn sessions_for_skill(invs: &[SkillInvocation], skill: &str) -> Vec<SessionEntry> {
    let mut sessions: BTreeMap<String, SessionEntry> = BTreeMap::new();
    for inv in invs {
        if inv.skill_name != skill {
            continue;
        }
        let entry = sessions
            .entry(inv.session_id.clone())
            .or_insert_with(|| SessionEntry {
                session_id: inv.session_id.clone(),
                project_path: inv.project_path.clone(),
                count: 0,
                first_ts: inv.timestamp,
                last_ts: inv.timestamp,
                subagent_count: 0,
            });
        entry.count += 1;
        if inv.origin == Origin::Subagent {
            entry.subagent_count += 1;
        }
        if inv.timestamp < entry.first_ts {
            entry.first_ts = inv.timestamp;
        }
        if inv.timestamp > entry.last_ts {
            entry.last_ts = inv.timestamp;
        }
    }
    let mut rows: Vec<SessionEntry> = sessions.into_values().collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count));
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Granularity {
    Day,
    Week,
}

fn period_key(ts: &DateTime<Utc>, granularity: Granularity) -> String {
    match granularity {
        Granularity::Week => {
            let date = ts.date_naive();
            let monday =
                date - chrono::Duration::days(date.weekday().num_days_from_monday() as i64);
            monday.format("%Y-%m-%d").to_string()
        }
        Granularity::Day => ts.date_naive().format("%Y-%m-%d").to_string(),
    }
}

/// Ordered (ascending) period -> invocation count, optionally scoped to one skill.
pub fn timeline(
    invs: &[SkillInvocation],
    skill: Option<&str>,
    granularity: Granularity,
) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for inv in invs {
        if let Some(skill) = skill
            && inv.skill_name != skill
        {
            continue;
        }
        *counts
            .entry(period_key(&inv.timestamp, granularity))
            .or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

#[derive(Debug, Clone)]
pub struct ProjectEntry {
    pub total: usize,
    pub top_skills: Vec<(String, usize)>,
}

/// Per-project totals plus each project's top-5 skills by count.
pub fn project_counts(invs: &[SkillInvocation]) -> BTreeMap<String, ProjectEntry> {
    let mut totals: BTreeMap<String, usize> = BTreeMap::new();
    let mut per_skill: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();

    for inv in invs {
        *totals.entry(inv.project_path.clone()).or_insert(0) += 1;
        *per_skill
            .entry(inv.project_path.clone())
            .or_default()
            .entry(inv.skill_name.clone())
            .or_insert(0) += 1;
    }

    let mut result = BTreeMap::new();
    for (project, total) in totals {
        let mut skills: Vec<(String, usize)> = per_skill
            .get(&project)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), *v)).collect())
            .unwrap_or_default();
        skills.sort_by(|a, b| b.1.cmp(&a.1));
        skills.truncate(5);
        result.insert(
            project,
            ProjectEntry {
                total,
                top_skills: skills,
            },
        );
    }
    result
}

/// Helper retained for clarity at call sites that need a bare date parse
/// (e.g. `--since`); not present in the Python reference as a separate fn.
#[allow(dead_code)]
pub fn parse_naive_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}
