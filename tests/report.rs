//! Integration tests for `report::build_report` — the "is my skill readily
//! being called from this directory?" survey — against
//! `tests/fixtures/scoped/`.

use chrono::{TimeZone, Utc};
use skillscope::report::build_report;
use skillscope::sessions::load_session_index;
use std::path::PathBuf;

fn scoped_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scoped")
}

const SCOPED_CWD: &str = "/Users/test/workspace/scoped-project";
const SESSION_ONE: &str = "1a2b3c4d-1111-4000-8000-000000000001";
const ZERO_INV_SESSION: &str = "ffffffff-0000-4000-8000-00000000000f";

#[test]
fn report_counts_all_sessions_including_zero_invocation_ones() {
    let dir = scoped_fixtures_dir();
    let index = load_session_index(&dir);
    let report = build_report(&dir, SCOPED_CWD, None, None, &index);

    assert_eq!(report.sessions_total, 3);
    assert_eq!(report.sessions_with_invocations, 2);
    // The zero-invocation session appears in the per-session profile.
    let zero = report
        .sessions
        .iter()
        .find(|s| s.session_id == ZERO_INV_SESSION)
        .expect("zero-invocation session profiled");
    assert_eq!(zero.total_invocations, 0);
    assert_eq!(zero.distinct_skills, 0);
    assert_eq!(zero.label, "Zero-invocation chat session");
}

#[test]
fn per_skill_usage_aggregates_trigger_context_and_session_reach() {
    let dir = scoped_fixtures_dir();
    let index = load_session_index(&dir);
    let report = build_report(&dir, SCOPED_CWD, None, None, &index);

    let names: Vec<&str> = report
        .skills
        .iter()
        .map(|s| s.skill_name.as_str())
        .collect();
    assert_eq!(names.len(), 4); // worktree, cve-lookup, github-cli, terraform

    let worktree = report
        .skills
        .iter()
        .find(|s| s.skill_name == "worktree")
        .unwrap();
    assert_eq!(worktree.total, 1);
    assert_eq!(worktree.user_slash, 1);
    assert_eq!(worktree.claude_proactive, 0);
    assert_eq!(worktree.sessions, 1);

    let github_cli = report
        .skills
        .iter()
        .find(|s| s.skill_name == "github-cli")
        .unwrap();
    assert_eq!(github_cli.subagent, 1);
    assert_eq!(github_cli.claude_proactive, 1);
}

#[test]
fn focus_reports_invoked_in_n_of_m_sessions_with_trigger_breakdown() {
    let dir = scoped_fixtures_dir();
    let index = load_session_index(&dir);
    let report = build_report(&dir, SCOPED_CWD, None, Some("cve-lookup"), &index);

    let focus = report.focus.expect("focus section present");
    assert_eq!(focus.skill_name, "cve-lookup");
    assert_eq!(focus.sessions_invoked, 1);
    assert_eq!(focus.sessions_total, 3);
    assert_eq!(focus.rows.len(), 1);
    let row = &focus.rows[0];
    assert_eq!(row.session_id, SESSION_ONE);
    assert_eq!(row.count, 1);
    assert_eq!(row.user_slash, 0);
    assert_eq!(row.claude_proactive, 1);
}

#[test]
fn focus_on_never_invoked_skill_yields_zero_of_m() {
    let dir = scoped_fixtures_dir();
    let index = load_session_index(&dir);
    let report = build_report(&dir, SCOPED_CWD, None, Some("never-used-skill"), &index);
    let focus = report.focus.unwrap();
    assert_eq!(focus.sessions_invoked, 0);
    assert_eq!(focus.sessions_total, 3);
    assert!(focus.rows.is_empty());
}

#[test]
fn since_filters_sessions_by_last_activity() {
    let dir = scoped_fixtures_dir();
    let index = load_session_index(&dir);
    // Cutoff after SESSION_ONE's index `modified` (2026-03-01T09:30) but
    // before ZERO_INV_SESSION's (2026-03-03T08:30). SESSION_TWO is
    // unindexed -> mtime (checkout time, recent) keeps it in.
    let since = Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap();
    let report = build_report(&dir, SCOPED_CWD, Some(since), None, &index);
    assert_eq!(report.sessions_total, 2);
    assert!(!report.sessions.iter().any(|s| s.session_id == SESSION_ONE));
    // SESSION_ONE's skills drop out of the aggregate with it.
    assert!(!report.skills.iter().any(|s| s.skill_name == "worktree"));
}

#[test]
fn unknown_cwd_yields_empty_report() {
    let dir = scoped_fixtures_dir();
    let index = load_session_index(&dir);
    let report = build_report(&dir, "/no/such/dir", None, None, &index);
    assert_eq!(report.sessions_total, 0);
    assert!(report.skills.is_empty());
    assert!(report.sessions.is_empty());
}
