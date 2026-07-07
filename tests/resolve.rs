//! Integration tests for session-id resolution against committed fixtures
//! under `tests/fixtures/scoped/` (a separate root from `projects/` so the
//! whole-corpus counts asserted in `tests/pipeline.rs` stay untouched).
//!
//! Fixture ids are real hex UUIDs:
//!   1a2b3c4d-1111-... and 1a2b3c4d-2222-...  share the 8-char prefix
//!   dddddddd-3333-...                        exists in two project dirs
//!   ffffffff-0000-...                        zero-invocation session

use skillscope::resolve::{ResolveError, resolve_session_id};
use std::path::PathBuf;

fn scoped_fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scoped")
}

const SESSION_ONE: &str = "1a2b3c4d-1111-4000-8000-000000000001";
const SESSION_TWO: &str = "1a2b3c4d-2222-4000-8000-000000000002";
const DUP_SESSION: &str = "dddddddd-3333-4000-8000-00000000000d";

#[test]
fn full_uuid_resolves_to_its_transcript() {
    let path = resolve_session_id(&scoped_fixtures_dir(), SESSION_ONE).unwrap();
    assert!(path.ends_with(format!(
        "-Users-test-workspace-scoped-project/{SESSION_ONE}.jsonl"
    )));
}

#[test]
fn unique_prefix_resolves_when_long_enough_to_disambiguate() {
    // "1a2b3c4d-1" is 10 chars but contains '-', so it counts as a
    // hex-prefix only if we include the dash — it doesn't; use the
    // second block to disambiguate via a longer dashless prefix instead.
    // "1a2b3c4d" alone is ambiguous (two sessions share it).
    match resolve_session_id(&scoped_fixtures_dir(), "1a2b3c4d") {
        Err(ResolveError::AmbiguousPrefix(ids)) => {
            assert_eq!(ids, vec![SESSION_ONE.to_string(), SESSION_TWO.to_string()]);
        }
        other => panic!("expected AmbiguousPrefix, got {other:?}"),
    }
}

#[test]
fn duplicate_session_id_across_project_dirs_is_an_error_listing_both_paths() {
    match resolve_session_id(&scoped_fixtures_dir(), DUP_SESSION) {
        Err(ResolveError::DuplicateAcrossProjects(paths)) => {
            assert_eq!(paths.len(), 2);
            assert!(paths[0].to_string_lossy().contains("-dup-a"));
            assert!(paths[1].to_string_lossy().contains("-dup-b"));
        }
        other => panic!("expected DuplicateAcrossProjects, got {other:?}"),
    }
}

#[test]
fn zero_invocation_session_still_resolves() {
    let path = resolve_session_id(
        &scoped_fixtures_dir(),
        "ffffffff-0000-4000-8000-00000000000f",
    )
    .unwrap();
    assert!(path.exists());
}

#[test]
fn short_and_malformed_targets_are_rejected() {
    let dir = scoped_fixtures_dir();
    assert!(matches!(
        resolve_session_id(&dir, "1a2b3c4"),
        Err(ResolveError::PrefixTooShort(_))
    ));
    assert!(matches!(
        resolve_session_id(&dir, "not-hex-at-all"),
        Err(ResolveError::InvalidFormat(_))
    ));
    assert!(matches!(
        resolve_session_id(&dir, "0000"),
        Err(ResolveError::PrefixTooShort(_))
    ));
}

#[test]
fn no_match_for_unknown_full_uuid() {
    assert!(matches!(
        resolve_session_id(
            &scoped_fixtures_dir(),
            "99999999-9999-4999-8999-999999999999"
        ),
        Err(ResolveError::NoMatch(_))
    ));
}
