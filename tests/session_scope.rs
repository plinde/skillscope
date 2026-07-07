//! Integration tests for the session-scoped pipeline: resolve ->
//! iter_invocations_for_session -> scoped discovery, against
//! `tests/fixtures/scoped/`.

use skillscope::models::Origin;
use skillscope::resolve::resolve_session_id;
use skillscope::sessions::load_session_index;
use skillscope::sessionscan::discover_sessions_for_cwd;
use skillscope::{parser, sessionscan};
use std::path::PathBuf;

fn scoped_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scoped")
}

const SESSION_ONE: &str = "1a2b3c4d-1111-4000-8000-000000000001";
const SESSION_TWO: &str = "1a2b3c4d-2222-4000-8000-000000000002";
const ZERO_INV_SESSION: &str = "ffffffff-0000-4000-8000-00000000000f";
const SCOPED_CWD: &str = "/Users/test/workspace/scoped-project";

#[test]
fn scoped_parse_covers_main_and_own_subagents_but_not_sibling_sessions() {
    let transcript = resolve_session_id(&scoped_fixtures_dir(), SESSION_ONE).unwrap();
    let invs = parser::iter_invocations_for_session(&transcript);

    let names: Vec<&str> = invs.iter().map(|i| i.skill_name.as_str()).collect();
    assert!(names.contains(&"worktree"));
    assert!(names.contains(&"cve-lookup"));
    // The subagent invocation under SESSION_ONE's own session dir.
    let sub = invs.iter().find(|i| i.skill_name == "github-cli").unwrap();
    assert_eq!(sub.origin, Origin::Subagent);
    // SESSION_TWO's /terraform lives in the same project dir but a
    // different transcript — must NOT leak into this scope.
    assert!(!names.contains(&"terraform"));
    assert_eq!(invs.len(), 3);
}

#[test]
fn scoped_parse_of_zero_invocation_session_is_empty_not_an_error() {
    let transcript = resolve_session_id(&scoped_fixtures_dir(), ZERO_INV_SESSION).unwrap();
    assert!(parser::iter_invocations_for_session(&transcript).is_empty());
}

#[test]
fn discovery_lists_every_session_for_the_cwd_including_zero_invocation() {
    let index = load_session_index(&scoped_fixtures_dir());
    let sessions = discover_sessions_for_cwd(&scoped_fixtures_dir(), SCOPED_CWD, &index);
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&SESSION_ONE));
    assert!(ids.contains(&SESSION_TWO));
    assert!(ids.contains(&ZERO_INV_SESSION));
}

#[test]
fn discovery_prefers_index_modified_and_sorts_desc() {
    let index = load_session_index(&scoped_fixtures_dir());
    let sessions = discover_sessions_for_cwd(&scoped_fixtures_dir(), SCOPED_CWD, &index);
    // ZERO_INV_SESSION has the newest index `modified` (2026-03-03) and
    // SESSION_ONE the older one (2026-03-01); SESSION_TWO is unindexed so
    // it falls back to file mtime (checkout-time, i.e. recent) and sorts
    // first. The two indexed sessions must be ordered by `modified` desc.
    let pos = |id: &str| sessions.iter().position(|s| s.session_id == id).unwrap();
    assert!(pos(ZERO_INV_SESSION) < pos(SESSION_ONE));

    // Index metadata flows through to the summary.
    let one = &sessions[pos(SESSION_ONE)];
    assert_eq!(one.label, "Scoped worktree and CVE lookup");
    assert_eq!(one.git_branch.as_deref(), Some("scoped-branch"));
    assert_eq!(one.project_path, SCOPED_CWD);
    assert_eq!(one.last_turn.to_rfc3339(), "2026-03-01T09:30:00+00:00");
}

#[test]
fn encode_matches_the_fixture_dir_name() {
    assert_eq!(
        sessionscan::encode_cwd_to_dirname(SCOPED_CWD),
        "-Users-test-workspace-scoped-project"
    );
}
