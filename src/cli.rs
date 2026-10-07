//! skillscope CLI: clap subcommands over parsed skill invocations.
//!
//! Mirrors `skillscope/cli.py`'s subcommands (summary, sessions, timeline,
//! projects, fidelity, export) plus the Rust rewrite's `--origin` filter and
//! subagent-origin columns/fields.

use crate::aggregate::{self, Granularity};
use crate::fidelity::run_fidelity;
use crate::harness;
use crate::models::{
    Harness, Origin, ReadExtent, RefRead, SkillInvocation, TriggerCounts, TriggerType,
};
use crate::parser::iter_invocations;
use crate::refreads;
use crate::sessions::{load_session_index, session_branch, session_label};
use crate::skilltree::{self, Finding, Severity, SkillTree};
use chrono::{DateTime, NaiveDate, TimeZone, Utc};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn default_projects_dir() -> PathBuf {
    home_dir().join(".claude").join("projects")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OriginFilter {
    Main,
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, ValueEnum)]
pub enum HarnessFilter {
    #[default]
    Claude,
    Codex,
    Pi,
    Opencode,
    All,
}

impl HarnessFilter {
    pub fn includes(self, harness: Harness) -> bool {
        match self {
            HarnessFilter::All => true,
            HarnessFilter::Claude => harness == Harness::Claude,
            HarnessFilter::Codex => harness == Harness::Codex,
            HarnessFilter::Pi => harness == Harness::Pi,
            HarnessFilter::Opencode => harness == Harness::Opencode,
        }
    }
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

    /// Which agent harness's history to read. `claude` (default) reads
    /// Claude Code transcripts only; `codex`, `pi` and `opencode` read that
    /// harness's session store; `all` reads every one that exists.
    #[arg(long, value_enum, global = true, default_value_t = HarnessFilter::Claude)]
    pub harness: HarnessFilter,

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
    /// Lint installed skills against the reference-file rules
    /// (nested-ref, ref-over-100, orphan-file, body-over-500)
    Lint {
        /// Optional skill to lint (default: every installed skill)
        skill: Option<String>,
        /// Skill root to scan (repeatable; default as for `inventory`)
        #[arg(long = "skills-dir")]
        skills_dirs: Vec<PathBuf>,
        /// Exit 1 when there is any finding
        #[arg(long)]
        fail: bool,
    },
    /// Per bundled file: depth, size, TOC, and how transcripts read it
    /// (full, partial, search), by which model
    Refs {
        /// Optional skill to show (default: every skill with bundled files)
        skill: Option<String>,
        /// Skill root to scan (repeatable; default as for `inventory`)
        #[arg(long = "skills-dir")]
        skills_dirs: Vec<PathBuf>,
        /// One row per (file, model) instead of per file
        #[arg(long)]
        by_model: bool,
        /// Print the matched reads themselves as JSON lines (one per tool
        /// call) instead of the table
        #[arg(long)]
        reads: bool,
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

/// Every invocation the selected harnesses recorded, before `--since` /
/// `--origin` filtering.
pub fn harness_invocations(cli: &Cli) -> Vec<SkillInvocation> {
    let mut invs = Vec::new();
    if cli.harness.includes(Harness::Claude) {
        invs.extend(iter_invocations(&cli.resolved_projects_dir()));
    }
    let home = home_dir();
    if cli.harness.includes(Harness::Codex) {
        invs.extend(harness::iter_codex_invocations(
            &home.join(".codex/sessions"),
        ));
    }
    if cli.harness.includes(Harness::Pi) {
        invs.extend(harness::iter_pi_invocations(
            &home.join(".pi/agent/sessions"),
        ));
    }
    if cli.harness.includes(Harness::Opencode) {
        let db = home.join(".local/share/opencode/opencode.db");
        match harness::iter_opencode_invocations(&db) {
            Ok(found) => invs.extend(found),
            Err(e) => eprintln!("skillscope: skipping opencode: {e}"),
        }
    }
    invs
}

fn load_invocations(cli: &Cli) -> Vec<SkillInvocation> {
    let since = cli.resolved_since();
    let mut invs = harness_invocations(cli);
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
    #[serde(flatten)]
    triggers: TriggerCounts,
    subagent: usize,
    first_seen: String,
    last_seen: String,
}

pub fn cmd_summary(cli: &Cli) {
    let invs = load_invocations(cli);
    let counts = aggregate::skill_counts(&invs);
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.1.total));

    if cli.json {
        let json_map: std::collections::BTreeMap<String, SkillCountsJson> = rows
            .into_iter()
            .map(|(name, s)| {
                (
                    name,
                    SkillCountsJson {
                        total: s.total,
                        triggers: s.triggers,
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
        "{:<30} {:>7} {:>12} {:>11} {:>17} {:>12} {:>10} {:>12} {:>12}",
        "Skill",
        "Total",
        "User /slash",
        "User named",
        "Claude proactive",
        "Direct read",
        "Subagent",
        "First seen",
        "Last seen"
    );
    for (name, stats) in rows {
        println!(
            "{:<30} {:>7} {:>12} {:>11} {:>17} {:>12} {:>10} {:>12} {:>12}",
            name,
            stats.total,
            stats.triggers.user_slash,
            stats.triggers.user_named,
            stats.triggers.claude_proactive,
            stats.triggers.direct_read,
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
    rows.sort_by_key(|a| std::cmp::Reverse(a.1.total));

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
    println!("Trigger context: slash = typed /command; named = Skill tool call the user asked for");
    println!(
        "as /skill or $skill in prose; proactive = Skill tool call nobody asked for (keyword/"
    );
    println!("description trigger); read = SKILL.md loaded with Read/Bash instead of the Skill");
    println!("tool; subagent = fired inside a subagent.");

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
                "{:<50} {:>6} {:>7} {:>7} {:>10} {:>6} {:>9} {:>17}",
                "Session", "Count", "Slash", "Named", "Proactive", "Read", "Subagent", "Last"
            );
            for row in &focus.rows {
                let label: String = row.label.chars().take(50).collect();
                println!(
                    "{:<50} {:>6} {:>7} {:>7} {:>10} {:>6} {:>9} {:>17}",
                    label,
                    row.count,
                    row.triggers.user_slash,
                    row.triggers.user_named,
                    row.triggers.claude_proactive,
                    row.triggers.direct_read,
                    row.subagent,
                    row.last_ts.format("%Y-%m-%d %H:%M"),
                );
            }
        }
    }

    println!();
    println!("Per-skill usage across these sessions");
    println!(
        "{:<30} {:>6} {:>7} {:>7} {:>10} {:>6} {:>9} {:>9} {:>12} {:>12}",
        "Skill",
        "Total",
        "Slash",
        "Named",
        "Proactive",
        "Read",
        "Subagent",
        "Sessions",
        "First seen",
        "Last seen"
    );
    for s in &report.skills {
        println!(
            "{:<30} {:>6} {:>7} {:>7} {:>10} {:>6} {:>9} {:>9} {:>12} {:>12}",
            s.skill_name,
            s.total,
            s.triggers.user_slash,
            s.triggers.user_named,
            s.triggers.claude_proactive,
            s.triggers.direct_read,
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
            "  Invocations: {} total ({} user-slash, {} user-named, {} claude-proactive, {} direct-read, {} in subagents)",
            row.total_invocations,
            row.triggers.user_slash,
            row.triggers.user_named,
            row.triggers.claude_proactive,
            row.triggers.direct_read,
            row.subagent
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
                        TriggerType::UserNamed =>
                            "Skill tool call the user asked for by /name or $name",
                        TriggerType::ClaudeProactive =>
                            "model-invoked via Skill tool — keyword/description trigger",
                        TriggerType::DirectRead => "SKILL.md read directly, not via the Skill tool",
                    }
                );
                println!("    Origin:    {}", last.origin);
                println!("    Harness:   {}", last.harness);
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
        "{:<34} {:<7} {:>6} {:>7} {:>7} {:>10} {:>6} {:>9} {:>17}  Last session",
        "Skill",
        "Source",
        "Total",
        "Slash",
        "Named",
        "Proactive",
        "Read",
        "Subagent",
        "Last invoked"
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
            "{:<34} {:<7} {:>6} {:>7} {:>7} {:>10} {:>6} {:>9} {:>17}  {}",
            format!("{}{sym}", row.skill.name),
            row.skill.source,
            row.total_invocations,
            row.triggers.user_slash,
            row.triggers.user_named,
            row.triggers.claude_proactive,
            row.triggers.direct_read,
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
    harness: Harness,
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
            harness: inv.harness,
        };
        if writeln!(lock, "{}", serde_json::to_string(&json_inv).unwrap()).is_err() {
            // Broken pipe (e.g. piped into `head`) — matches Python's
            // BrokenPipeError handling: stop writing, exit quietly.
            break;
        }
    }
}

/// Every bundled-file read the selected harnesses recorded, after
/// `--since` / `--origin`. opencode has no file-read records.
fn load_ref_reads(cli: &Cli) -> Vec<RefRead> {
    let mut reads = Vec::new();
    if cli.harness.includes(Harness::Claude) {
        reads.extend(refreads::iter_claude_ref_reads(
            &cli.resolved_projects_dir(),
        ));
    }
    let home = home_dir();
    if cli.harness.includes(Harness::Codex) {
        reads.extend(refreads::iter_codex_ref_reads(
            &home.join(".codex/sessions"),
        ));
    }
    if cli.harness.includes(Harness::Pi) {
        reads.extend(refreads::iter_pi_ref_reads(
            &home.join(".pi/agent/sessions"),
        ));
    }
    if let Some(since) = cli.resolved_since() {
        reads.retain(|r| r.timestamp >= since);
    }
    if let Some(origin_filter) = cli.origin {
        reads.retain(|r| match origin_filter {
            OriginFilter::Main => r.origin == Origin::Main,
            OriginFilter::Subagent => r.origin == Origin::Subagent,
        });
    }
    reads
}

/// Installed skills (optionally one) walked into trees, with each skill's
/// directory name (what transcript paths carry). Exits 1 when a named
/// skill isn't installed.
fn installed_trees(skill: Option<&str>, skills_dirs: &[PathBuf]) -> Vec<(String, SkillTree)> {
    let dirs = (!skills_dirs.is_empty()).then_some(skills_dirs);
    let installed = crate::inventory::inventory_skills(dirs);
    let trees: Vec<(String, SkillTree)> = installed
        .iter()
        .filter(|s| skill.is_none_or(|k| s.name == k))
        .filter_map(|s| {
            let dir_name = s
                .resolved_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(&s.name)
                .to_string();
            skilltree::walk_skill(&s.name, &s.resolved_path).map(|t| (dir_name, t))
        })
        .collect();
    if let Some(skill) = skill
        && trees.is_empty()
    {
        eprintln!("Skill '{skill}' is not installed in the scanned skill roots.");
        std::process::exit(1);
    }
    trees
}

pub fn cmd_lint(cli: &Cli, skill: Option<&str>, skills_dirs: &[PathBuf], fail: bool) {
    let trees = installed_trees(skill, skills_dirs);
    let findings: Vec<Finding> = trees.iter().flat_map(|(_, t)| skilltree::lint(t)).collect();

    if cli.json {
        print_json(&findings);
    } else {
        println!(
            "Skill lint — {} skills, {} findings",
            trees.len(),
            findings.len()
        );
        if !findings.is_empty() {
            println!(
                "{:<30} {:<17} {:<6} {:<55} Source",
                "Skill", "Rule", "Level", "Detail"
            );
        }
        for f in &findings {
            println!(
                "{:<30} {:<17} {:<6} {:<55} {}",
                f.skill,
                f.code,
                match f.severity {
                    Severity::Error => "error",
                    Severity::Warn => "warn",
                },
                f.detail,
                f.source
            );
        }
    }
    if fail && !findings.is_empty() {
        std::process::exit(1);
    }
}

#[derive(Serialize, Default)]
struct RefRow {
    skill: String,
    file: String,
    /// Steps from SKILL.md; None = orphan.
    depth: Option<usize>,
    lines: usize,
    has_toc: bool,
    /// Present only with --by-model.
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    reads: usize,
    full: usize,
    partial: usize,
    search: usize,
    unknown: usize,
    /// partial / (full + partial) as a percentage; None without content reads.
    partial_pct: Option<f64>,
    models: Vec<String>,
    last_read: Option<DateTime<Utc>>,
    /// The skill was used in the window but this file was never read.
    never_read: bool,
}

fn ref_row(base: RefRow, reads: &[&RefRead]) -> RefRow {
    let mut row = base;
    let mut models = std::collections::BTreeSet::new();
    for r in reads {
        row.reads += 1;
        match r.extent {
            ReadExtent::Full => row.full += 1,
            ReadExtent::Partial { .. } => row.partial += 1,
            ReadExtent::Search => row.search += 1,
            ReadExtent::Unknown => row.unknown += 1,
        }
        models.insert(r.model.clone().unwrap_or_else(|| "?".to_string()));
        row.last_read = row.last_read.max(Some(r.timestamp));
    }
    let content = row.full + row.partial;
    row.partial_pct = (content > 0).then(|| 100.0 * row.partial as f64 / content as f64);
    row.models = models.into_iter().collect();
    row
}

pub fn cmd_refs(
    cli: &Cli,
    skill: Option<&str>,
    skills_dirs: &[PathBuf],
    by_model: bool,
    raw_reads: bool,
) {
    let trees = installed_trees(skill, skills_dirs);
    let reads = load_ref_reads(cli);
    let used: std::collections::HashSet<String> = load_invocations(cli)
        .into_iter()
        .map(|i| i.skill_name)
        .collect();

    let mut rows: Vec<RefRow> = Vec::new();
    for (dir_name, tree) in &trees {
        let skill_used = used.contains(&tree.skill) || used.contains(dir_name);
        // files[0] is SKILL.md, whose reads are invocations.
        for f in tree.files.iter().skip(1) {
            let file_reads: Vec<&RefRead> = reads
                .iter()
                .filter(|r| {
                    (r.skill_name == tree.skill || r.skill_name == *dir_name)
                        && r.rel_path == f.rel_path
                })
                .collect();
            if raw_reads {
                for r in &file_reads {
                    println!("{}", serde_json::to_string(r).unwrap());
                }
                continue;
            }
            let base = || RefRow {
                skill: tree.skill.clone(),
                file: f.rel_path.clone(),
                depth: f.depth,
                lines: f.lines,
                has_toc: f.has_toc,
                never_read: skill_used && file_reads.is_empty(),
                ..Default::default()
            };
            if by_model && !file_reads.is_empty() {
                let mut by: BTreeMap<String, Vec<&RefRead>> = BTreeMap::new();
                for r in &file_reads {
                    by.entry(r.model.clone().unwrap_or_else(|| "?".to_string()))
                        .or_default()
                        .push(r);
                }
                for (model, rs) in by {
                    let row = RefRow {
                        model: Some(model),
                        ..base()
                    };
                    rows.push(ref_row(row, &rs));
                }
            } else {
                rows.push(ref_row(base(), &file_reads));
            }
        }
    }
    if raw_reads {
        return;
    }

    if cli.json {
        print_json(&rows);
        return;
    }
    let window = cli
        .since
        .as_deref()
        .map(|s| format!(", window {s}"))
        .unwrap_or_default();
    println!(
        "Bundled-file reads — {} rows, {} reads (harness {:?}{window})",
        rows.len(),
        rows.iter().map(|r| r.reads).sum::<usize>(),
        cli.harness
    );
    println!(
        "{:<22} {:<40} {:>5} {:>5} {:>3} {:>5} {:>4} {:>4} {:>4} {:>5} {:>5} {:>16}  {}",
        "Skill",
        "File",
        "Depth",
        "Lines",
        "TOC",
        "Reads",
        "Full",
        "Part",
        "Srch",
        "Other",
        "Part%",
        "Last read",
        if by_model { "Model" } else { "Models" }
    );
    for r in &rows {
        let flag = if r.never_read { " !" } else { "" };
        println!(
            "{:<22} {:<40} {:>5} {:>5} {:>3} {:>5} {:>4} {:>4} {:>4} {:>5} {:>5} {:>16}  {}",
            r.skill,
            format!("{}{flag}", r.file),
            r.depth.map_or("orph".to_string(), |d| d.to_string()),
            r.lines,
            if r.has_toc { "y" } else { "-" },
            r.reads,
            r.full,
            r.partial,
            r.search,
            r.unknown,
            r.partial_pct.map_or("-".to_string(), |p| format!("{p:.0}")),
            r.last_read.map_or("never".to_string(), |t| t
                .format("%Y-%m-%d %H:%M")
                .to_string()),
            match &r.model {
                Some(m) => m.clone(),
                None => r.models.join(","),
            },
        );
    }
    println!();
    println!(
        "Depth: steps from SKILL.md (orph = unreachable). Part% = partial / (full + partial)."
    );
    println!("Other = extent unknown (script runs, pagers).");
    println!("! = skill used in the window but this file never read.");
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
            harness: HarnessFilter::Claude,
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
