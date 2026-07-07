//! skillscope CLI: clap subcommands over parsed skill invocations.
//!
//! Mirrors `skillscope/cli.py`'s subcommands (summary, sessions, timeline,
//! projects, fidelity, export) plus the Rust rewrite's `--origin` filter and
//! subagent-origin columns/fields.

use crate::aggregate::{self, Granularity};
use crate::fidelity::run_fidelity;
use crate::models::{Origin, SkillInvocation, TriggerType};
use crate::parser::iter_invocations;
use crate::sessions::{load_session_index, session_branch, session_label};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::path::PathBuf;

fn default_projects_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"));
    home.join(".claude").join("projects")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OriginFilter {
    Main,
    Subagent,
}

#[derive(Parser, Debug)]
#[command(
    name = "skillscope",
    about = "Claude Code skill-invocation analytics (local JSONL transcripts only).",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Directory containing Claude Code project transcripts.
    #[arg(long, global = true)]
    pub projects_dir: Option<PathBuf>,

    /// Only include invocations on/after this date. Accepts YYYY-MM-DD or a
    /// relative window like `7d` / `30d`.
    #[arg(long, global = true)]
    pub since: Option<String>,

    /// Emit machine-readable JSON output.
    #[arg(long, global = true)]
    pub json: bool,

    /// Restrict to invocations from main-session or subagent transcripts.
    #[arg(long, value_enum, global = true)]
    pub origin: Option<OriginFilter>,

    /// Session scope: `.` picks a session for the current directory via
    /// fzf; a full UUID or >=8-char hex prefix opens that session directly.
    #[arg(value_name = "TARGET", conflicts_with = "command")]
    pub target: Option<String>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Per-skill counts and trigger breakdown
    Summary,
    /// Session drill-down for one skill
    Sessions { skill: String },
    /// Time-series of invocations
    Timeline {
        skill: Option<String>,
        #[arg(long)]
        week: bool,
    },
    /// Per-project skill usage breakdown
    Projects,
    /// Trigger-fidelity report
    Fidelity,
    /// JSON-lines export of normalized invocations
    Export,
    /// Survey skill usage across sessions started from one directory
    Report {
        /// Optional skill to focus on ("is xyz readily being called?")
        skill: Option<String>,
        /// Working directory the sessions were started from
        /// (default: current directory)
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// Installed-skill inventory joined against invocation history
    Inventory {
        /// Optional skill to show in detail ("when did abc last run?")
        skill: Option<String>,
        /// Skill root to scan (repeatable; default: ~/.agents/skills,
        /// ~/.claude/skills, plus plugin marketplaces)
        #[arg(long = "skills-dir")]
        skills_dirs: Vec<PathBuf>,
    },
}

impl Cli {
    pub fn resolved_projects_dir(&self) -> PathBuf {
        self.projects_dir
            .clone()
            .unwrap_or_else(default_projects_dir)
    }

    /// Parse `--since`: `YYYY-MM-DD` or relative `<N>d` (e.g. `7d`, `30d`).
    pub fn resolved_since(&self) -> Option<DateTime<Utc>> {
        let raw = self.since.as_ref()?;
        if let Some(days_str) = raw.strip_suffix('d')
            && let Ok(days) = days_str.parse::<i64>()
        {
            return Some(Utc::now() - chrono::Duration::days(days));
        }
        NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .map(|dt| Utc.from_utc_datetime(&dt))
    }
}

fn load_invocations(cli: &Cli) -> Vec<SkillInvocation> {
    let since = cli.resolved_since();
    let mut invs = iter_invocations(&cli.resolved_projects_dir());
    if let Some(since) = since {
        invs.retain(|inv| inv.timestamp >= since);
    }
    if let Some(origin_filter) = cli.origin {
        invs.retain(|inv| match origin_filter {
            OriginFilter::Main => inv.origin == Origin::Main,
            OriginFilter::Subagent => inv.origin == Origin::Subagent,
        });
    }
    invs
}

fn print_json<T: Serialize>(data: &T) {
    println!("{}", serde_json::to_string_pretty(data).unwrap());
}

#[derive(Serialize)]
struct SkillCountsJson {
    total: usize,
    user_slash: usize,
    claude_proactive: usize,
    subagent: usize,
    first_seen: String,
    last_seen: String,
}

pub fn cmd_summary(cli: &Cli) {
    let invs = load_invocations(cli);
    let counts = aggregate::skill_counts(&invs);
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.total.cmp(&a.1.total));

    if cli.json {
        let json_map: std::collections::BTreeMap<String, SkillCountsJson> = rows
            .into_iter()
            .map(|(name, s)| {
                (
                    name,
                    SkillCountsJson {
                        total: s.total,
                        user_slash: s.user_slash,
                        claude_proactive: s.claude_proactive,
                        subagent: s.subagent,
                        first_seen: s.first_seen.to_rfc3339(),
                        last_seen: s.last_seen.to_rfc3339(),
                    },
                )
            })
            .collect();
        print_json(&json_map);
        return;
    }

    println!(
        "{:<30} {:>7} {:>12} {:>17} {:>10} {:>12} {:>12}",
        "Skill", "Total", "User /slash", "Claude proactive", "Subagent", "First seen", "Last seen"
    );
    for (name, stats) in rows {
        println!(
            "{:<30} {:>7} {:>12} {:>17} {:>10} {:>12} {:>12}",
            name,
            stats.total,
            stats.user_slash,
            stats.claude_proactive,
            stats.subagent,
            stats.first_seen.date_naive(),
            stats.last_seen.date_naive(),
        );
    }
}

#[derive(Serialize)]
struct SessionRowJson {
    session_id: String,
    project_path: String,
    count: usize,
    subagent_count: usize,
    first_ts: String,
    last_ts: String,
    label: String,
    git_branch: Option<String>,
}

pub fn cmd_sessions(cli: &Cli, skill: &str) {
    let invs = load_invocations(cli);
    let rows = aggregate::sessions_for_skill(&invs, skill);
    let index = load_session_index(&cli.resolved_projects_dir());

    if cli.json {
        let json_rows: Vec<SessionRowJson> = rows
            .into_iter()
            .map(|r| SessionRowJson {
                label: session_label(&r.session_id, &index),
                git_branch: session_branch(&r.session_id, &index),
                session_id: r.session_id,
                project_path: r.project_path,
                count: r.count,
                subagent_count: r.subagent_count,
                first_ts: r.first_ts.to_rfc3339(),
                last_ts: r.last_ts.to_rfc3339(),
            })
            .collect();
        print_json(&json_rows);
        return;
    }

    println!("Sessions invoking '{skill}'");
    println!(
        "{:<40} {:<30} {:>6} {:>10} {:>18} {:>18}",
        "Session", "Project", "Count", "Subagent", "First", "Last"
    );
    for row in rows {
        let label = session_label(&row.session_id, &index);
        println!(
            "{:<40} {:<30} {:>6} {:>10} {:>18} {:>18}",
            label,
            row.project_path,
            row.count,
            row.subagent_count,
            row.first_ts.format("%Y-%m-%dT%H:%M"),
            row.last_ts.format("%Y-%m-%dT%H:%M"),
        );
    }
}

pub fn cmd_timeline(cli: &Cli, skill: Option<&str>, week: bool) {
    let invs = load_invocations(cli);
    let granularity = if week {
        Granularity::Week
    } else {
        Granularity::Day
    };
    let series = aggregate::timeline(&invs, skill, granularity);

    if cli.json {
        let map: std::collections::BTreeMap<String, usize> = series.into_iter().collect();
        print_json(&map);
        return;
    }

    let title = match skill {
        Some(s) => format!(
            "Timeline ({}) for '{}'",
            if week { "week" } else { "day" },
            s
        ),
        None => format!("Timeline ({})", if week { "week" } else { "day" }),
    };
    println!("{title}");
    println!("{:<12} {:>6}", "Period", "Count");
    for (period, count) in series {
        println!("{period:<12} {count:>6}");
    }
}

#[derive(Serialize)]
struct ProjectRowJson {
    total: usize,
    top_skills: Vec<(String, usize)>,
}

pub fn cmd_projects(cli: &Cli) {
    let invs = load_invocations(cli);
    let counts = aggregate::project_counts(&invs);
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|a, b| b.1.total.cmp(&a.1.total));

    if cli.json {
        let json_map: std::collections::BTreeMap<String, ProjectRowJson> = rows
            .into_iter()
            .map(|(project, s)| {
                (
                    project,
                    ProjectRowJson {
                        total: s.total,
                        top_skills: s.top_skills,
                    },
                )
            })
            .collect();
        print_json(&json_map);
        return;
    }

    println!("Per-project skill usage");
    println!("{:<50} {:>6}  Top skills", "Project", "Total");
    for (project, stats) in rows {
        let top = stats
            .top_skills
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        println!("{:<50} {:>6}  {}", project, stats.total, top);
    }
}

#[derive(Serialize)]
struct FidelityFindingJson {
    skill_name: String,
    evidence: String,
    count: usize,
}

pub fn cmd_fidelity(cli: &Cli) {
    let report = run_fidelity(&cli.resolved_projects_dir(), None);

    if cli.json {
        #[derive(Serialize)]
        struct Report {
            under_triggered: Vec<FidelityFindingJson>,
            over_triggered: Vec<FidelityFindingJson>,
        }
        let to_json = |f: &crate::fidelity::FidelityFinding| FidelityFindingJson {
            skill_name: f.skill_name.clone(),
            evidence: f.evidence.clone(),
            count: f.count,
        };
        print_json(&Report {
            under_triggered: report.under_triggered.iter().map(to_json).collect(),
            over_triggered: report.over_triggered.iter().map(to_json).collect(),
        });
        return;
    }

    println!("Under-triggered skills (matched intent, never fired)");
    println!("{:<30} {:>6}  Evidence", "Skill", "Count");
    for item in &report.under_triggered {
        println!(
            "{:<30} {:>6}  {}",
            item.skill_name, item.count, item.evidence
        );
    }

    println!();
    println!("Over-triggered skills (fired on unrelated prompts)");
    println!("{:<30} {:>6}  Evidence", "Skill", "Count");
    for item in &report.over_triggered {
        println!(
            "{:<30} {:>6}  {}",
            item.skill_name, item.count, item.evidence
        );
    }
}

pub fn cmd_report(cli: &Cli, skill: Option<&str>, cwd: Option<&std::path::Path>) {
    let cwd_string = match cwd {
        Some(p) => p.to_string_lossy().to_string(),
        None => match std::env::current_dir() {
            Ok(d) => d.to_string_lossy().to_string(),
            Err(e) => {
                eprintln!("cannot determine current directory: {e}");
                std::process::exit(1);
            }
        },
    };
    let projects_dir = cli.resolved_projects_dir();
    let index = load_session_index(&projects_dir);
    let report = crate::report::build_report(
        &projects_dir,
        &cwd_string,
        cli.resolved_since(),
        skill,
        &index,
    );

    if report.sessions_total == 0 {
        eprintln!(
            "No Claude Code sessions found for {cwd_string}{}.",
            cli.since
                .as_deref()
                .map(|s| format!(" within --since {s}"))
                .unwrap_or_default()
        );
        std::process::exit(1);
    }

    if cli.json {
        print_json(&report);
        return;
    }

    let window = cli
        .since
        .as_deref()
        .map(|s| format!(", since {s}"))
        .unwrap_or_default();
    println!("Skill usage report — sessions started from {cwd_string}{window}");
    println!(
        "{} sessions ({} with skill invocations, {} without)",
        report.sessions_total,
        report.sessions_with_invocations,
        report.sessions_total - report.sessions_with_invocations
    );
    println!();
    println!("Trigger context: user-slash = typed /command; claude-proactive = model-invoked");
    println!(
        "via the Skill tool (keyword/description trigger); subagent = fired inside a subagent."
    );

    if let Some(focus) = &report.focus {
        println!();
        println!(
            "Focus: '{}' — invoked in {} of {} sessions",
            focus.skill_name, focus.sessions_invoked, focus.sessions_total
        );
        if focus.rows.is_empty() {
            println!("  (never invoked from this directory in the selected window)");
        } else {
            println!(
                "{:<50} {:>6} {:>7} {:>10} {:>9} {:>17}",
                "Session", "Count", "Slash", "Proactive", "Subagent", "Last"
            );
            for row in &focus.rows {
                let label: String = row.label.chars().take(50).collect();
                println!(
                    "{:<50} {:>6} {:>7} {:>10} {:>9} {:>17}",
                    label,
                    row.count,
                    row.user_slash,
                    row.claude_proactive,
                    row.subagent,
                    row.last_ts.format("%Y-%m-%d %H:%M"),
                );
            }
        }
    }

    println!();
    println!("Per-skill usage across these sessions");
    println!(
        "{:<30} {:>6} {:>7} {:>10} {:>9} {:>9} {:>12} {:>12}",
        "Skill", "Total", "Slash", "Proactive", "Subagent", "Sessions", "First seen", "Last seen"
    );
    for s in &report.skills {
        println!(
            "{:<30} {:>6} {:>7} {:>10} {:>9} {:>9} {:>12} {:>12}",
            s.skill_name,
            s.total,
            s.user_slash,
            s.claude_proactive,
            s.subagent,
            s.sessions,
            s.first_seen.date_naive(),
            s.last_seen.date_naive(),
        );
    }

    println!();
    println!("Per-session profile (most recent first)");
    println!(
        "{:<50} {:>6} {:>7} {:>17}  Top skills",
        "Session", "Invs", "Skills", "Last turn"
    );
    for row in &report.sessions {
        let label: String = row.label.chars().take(50).collect();
        let top = row
            .top_skills
            .iter()
            .map(|(name, count)| format!("{name} ({count})"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{:<50} {:>6} {:>7} {:>17}  {}",
            label,
            row.total_invocations,
            row.distinct_skills,
            row.last_turn.format("%Y-%m-%d %H:%M"),
            top,
        );
    }
}

pub fn cmd_inventory(cli: &Cli, skill: Option<&str>, skills_dirs: &[PathBuf]) {
    let dirs = if skills_dirs.is_empty() {
        None
    } else {
        Some(skills_dirs)
    };
    let installed = crate::inventory::inventory_skills(dirs);
    if installed.is_empty() {
        eprintln!("No installed skills found.");
        std::process::exit(1);
    }
    let invs = load_invocations(cli);
    let mut rows = crate::inventory::join_inventory(installed, &invs);

    if let Some(skill) = skill {
        rows.retain(|r| r.skill.name == skill);
        if rows.is_empty() {
            eprintln!("Skill '{skill}' is not installed in the scanned skill roots.");
            std::process::exit(1);
        }
    }

    if cli.json {
        print_json(&rows);
        return;
    }

    // Detail view for a single skill.
    if let Some(skill) = skill {
        let row = &rows[0];
        println!("Skill: {skill}");
        println!("  Source:      {}", row.skill.source);
        println!("  Path:        {}", row.skill.path.display());
        if row.skill.symlinked {
            println!("  Resolves to: {}", row.skill.resolved_path.display());
        }
        if !row.skill.description.is_empty() {
            let desc: String = row.skill.description.chars().take(200).collect();
            println!("  Description: {desc}");
        }
        println!(
            "  Invocations: {} total ({} user-slash, {} claude-proactive, {} in subagents)",
            row.total_invocations, row.user_slash, row.claude_proactive, row.subagent
        );
        match &row.last {
            Some(last) => {
                println!("  Last invoked:");
                println!("    Timestamp: {}", last.timestamp.to_rfc3339());
                println!("    Session:   {}", last.session_id);
                println!("    Project:   {}", last.project_path);
                println!(
                    "    Trigger:   {} ({})",
                    last.trigger_type,
                    match last.trigger_type {
                        TriggerType::UserSlash => "typed /command",
                        TriggerType::ClaudeProactive =>
                            "model-invoked via Skill tool — keyword/description trigger",
                    }
                );
                println!("    Origin:    {}", last.origin);
                if let Some(args) = &last.args {
                    let args: String = args.chars().take(120).collect();
                    println!("    Args:      {args}");
                }
            }
            None => println!(
                "  Last invoked: never{}",
                cli.since
                    .as_deref()
                    .map(|s| format!(" (within --since {s})"))
                    .unwrap_or_default()
            ),
        }
        return;
    }

    let never = rows.iter().filter(|r| r.last.is_none()).count();
    let window = cli
        .since
        .as_deref()
        .map(|s| format!(", window {s}"))
        .unwrap_or_default();
    println!(
        "Installed-skill inventory — {} skills ({} never invoked{window})",
        rows.len(),
        never
    );
    println!(
        "{:<34} {:<7} {:>6} {:>7} {:>10} {:>9} {:>17}  Last session",
        "Skill", "Source", "Total", "Slash", "Proactive", "Subagent", "Last invoked"
    );
    for row in &rows {
        let (last_ts, last_session) = match &row.last {
            Some(l) => (
                l.timestamp.format("%Y-%m-%d %H:%M").to_string(),
                l.session_id.clone(),
            ),
            None => ("never".to_string(), "-".to_string()),
        };
        let sym = if row.skill.symlinked { "@" } else { "" };
        println!(
            "{:<34} {:<7} {:>6} {:>7} {:>10} {:>9} {:>17}  {}",
            format!("{}{sym}", row.skill.name),
            row.skill.source,
            row.total_invocations,
            row.user_slash,
            row.claude_proactive,
            row.subagent,
            last_ts,
            last_session,
        );
    }
    println!();
    println!("@ = skill directory reached through a symlink");
}

#[derive(Serialize)]
struct InvocationJson {
    skill_name: String,
    trigger_type: TriggerType,
    session_id: String,
    project_path: String,
    timestamp: String,
    transcript_file: String,
    args: Option<String>,
    origin: Origin,
}

pub fn cmd_export(cli: &Cli) {
    let invs = load_invocations(cli);
    let stdout = std::io::stdout();
    use std::io::Write;
    let mut lock = stdout.lock();
    for inv in invs {
        let json_inv = InvocationJson {
            skill_name: inv.skill_name,
            trigger_type: inv.trigger_type,
            session_id: inv.session_id,
            project_path: inv.project_path,
            timestamp: inv.timestamp.to_rfc3339(),
            transcript_file: inv.transcript_file,
            args: inv.args,
            origin: inv.origin,
        };
        if writeln!(lock, "{}", serde_json::to_string(&json_inv).unwrap()).is_err() {
            // Broken pipe (e.g. piped into `head`) — matches Python's
            // BrokenPipeError handling: stop writing, exit quietly.
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cli_with_since(since: Option<&str>) -> Cli {
        Cli {
            projects_dir: None,
            since: since.map(String::from),
            json: false,
            origin: None,
            target: None,
            command: None,
        }
    }

    #[test]
    fn since_none_resolves_to_none() {
        assert!(cli_with_since(None).resolved_since().is_none());
    }

    #[test]
    fn since_absolute_date_parses_to_midnight_utc() {
        let cli = cli_with_since(Some("2026-01-15"));
        let resolved = cli.resolved_since().expect("should parse");
        assert_eq!(resolved.to_rfc3339(), "2026-01-15T00:00:00+00:00");
    }

    #[test]
    fn since_relative_days_resolves_to_now_minus_n_days() {
        let cli = cli_with_since(Some("7d"));
        let resolved = cli.resolved_since().expect("should parse");
        let expected = Utc::now() - chrono::Duration::days(7);
        let delta = (expected - resolved).num_seconds().abs();
        assert!(
            delta < 5,
            "expected resolved `since` within 5s of now-7d, got {delta}s off"
        );
    }

    #[test]
    fn since_relative_zero_days_resolves_to_now() {
        let cli = cli_with_since(Some("0d"));
        let resolved = cli.resolved_since().expect("should parse");
        let delta = (Utc::now() - resolved).num_seconds().abs();
        assert!(delta < 5);
    }

    #[test]
    fn since_invalid_forms_resolve_to_none() {
        assert!(cli_with_since(Some("garbage")).resolved_since().is_none());
        assert!(
            cli_with_since(Some("2026/01/15"))
                .resolved_since()
                .is_none()
        );
        assert!(cli_with_since(Some("d")).resolved_since().is_none());
        assert!(cli_with_since(Some("7x")).resolved_since().is_none());
        assert!(cli_with_since(Some("")).resolved_since().is_none());
    }

    #[test]
    fn since_negative_days_parses_as_a_negative_i64_and_resolves_into_the_future() {
        // "-7d".strip_suffix('d') == "-7", which parses as a valid i64, so
        // this resolves rather than falling through to None — documenting
        // actual behavior rather than asserting a stricter contract the
        // parser doesn't enforce.
        let cli = cli_with_since(Some("-7d"));
        let resolved = cli
            .resolved_since()
            .expect("negative day count still parses");
        assert!(resolved > Utc::now());
    }
}
