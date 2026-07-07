//! Integration tests: exercise parser -> sessions -> aggregate end-to-end
//! against small, committed fixture transcripts under `tests/fixtures/`,
//! rather than the live `~/.claude/projects` corpus (that's what
//! `scripts/parity.sh` / `make smoke` are for).
//!
//! Fixture layout mirrors real Claude Code project directories:
//!
//!   tests/fixtures/projects/
//!     -Users-test-workspace-demo-project/
//!       session-abc.jsonl              (main transcript, malformed lines,
//!                                        excluded command, noise prompt)
//!       sessions-index.json             (summary + firstPrompt + gitBranch)
//!       session-abc/subagents/agent-1.jsonl  (subagent-origin invocation)
//!     -Users-test-workspace-other-project/
//!       session-xyz.jsonl               (no cwd -> cwd-fallback decoding,
//!                                        no sessions-index.json -> UUID fallback)

use skillscope::models::{Origin, TriggerType};
use skillscope::sessions::{load_session_index, session_branch, session_label};
use skillscope::{parser, sessions};
use std::path::PathBuf;

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("projects")
}

#[test]
fn main_and_subagent_invocations_are_extracted_with_correct_origin() {
    let invs = parser::iter_invocations(&fixtures_dir());

    // 2 real invocations in the main transcript (worktree, cve-lookup) —
    // /clear is excluded, the Bash tool_use isn't a Skill call, and one
    // /terraform in the other project's main transcript makes 3 main total.
    let main_invs: Vec<_> = invs.iter().filter(|i| i.origin == Origin::Main).collect();
    let sub_invs: Vec<_> = invs
        .iter()
        .filter(|i| i.origin == Origin::Subagent)
        .collect();

    assert_eq!(sub_invs.len(), 1);
    assert_eq!(sub_invs[0].skill_name, "github-cli");
    assert_eq!(sub_invs[0].trigger_type, TriggerType::ClaudeProactive);

    assert_eq!(main_invs.len(), 3);
    let names: Vec<&str> = main_invs.iter().map(|i| i.skill_name.as_str()).collect();
    assert!(names.contains(&"worktree"));
    assert!(names.contains(&"cve-lookup"));
    assert!(names.contains(&"terraform"));
    // EXCLUDED_COMMANDS (clear, help) must never surface as invocations.
    assert!(!names.contains(&"clear"));
    assert!(!names.contains(&"help"));
}

#[test]
fn main_only_view_excludes_subagent_transcripts_entirely() {
    let invs = parser::iter_invocations_main_only(&fixtures_dir());
    assert!(invs.iter().all(|i| i.origin == Origin::Main));
    assert!(!invs.iter().any(|i| i.skill_name == "github-cli"));
}

#[test]
fn worktree_invocation_carries_its_command_args() {
    let invs = parser::iter_invocations_main_only(&fixtures_dir());
    let worktree = invs
        .iter()
        .find(|i| i.skill_name == "worktree")
        .expect("worktree invocation present");
    assert_eq!(worktree.args.as_deref(), Some("feature-branch"));
    assert_eq!(worktree.trigger_type, TriggerType::UserSlash);
}

#[test]
fn cwd_present_is_used_verbatim_and_absent_cwd_falls_back_to_decoded_dir_name() {
    let invs = parser::iter_invocations_main_only(&fixtures_dir());

    let worktree = invs.iter().find(|i| i.skill_name == "worktree").unwrap();
    assert_eq!(
        worktree.project_path,
        "/Users/test/workspace/demo-project"
    );

    // session-xyz.jsonl lines have no "cwd" field at all, so the project
    // path must come from decoding the containing directory name. The
    // decode is lossy (dashes in real path components get split too) —
    // "-Users-test-workspace-other-project" decodes to .../other/project,
    // not .../other-project. That's expected fallback behavior, not a bug.
    let terraform = invs.iter().find(|i| i.skill_name == "terraform").unwrap();
    assert_eq!(
        terraform.project_path,
        "/Users/test/workspace/other/project"
    );
}

#[test]
fn malformed_and_incomplete_lines_do_not_break_extraction_of_valid_ones() {
    // session-abc.jsonl opens with a non-JSON line and ends with a truncated
    // JSON object; both must be skipped without affecting the well-formed
    // lines in between.
    let invs = parser::iter_invocations_main_only(&fixtures_dir());
    assert!(invs.iter().any(|i| i.skill_name == "worktree"));
    assert!(invs.iter().any(|i| i.skill_name == "cve-lookup"));
}

#[test]
fn noise_prompts_are_excluded_but_real_prompts_survive() {
    let prompts = parser::iter_user_prompts(&fixtures_dir());
    // The isMeta:true line must never appear as a real user prompt.
    assert!(
        !prompts
            .iter()
            .any(|p| p.text.contains("synthetic conversation title generator"))
    );
    // The genuine free-text prompt in session-abc must survive.
    assert!(
        prompts
            .iter()
            .any(|p| p.text.contains("set up a new worktree"))
    );
}

#[test]
fn session_index_join_uses_full_fallback_chain() {
    let index = load_session_index(&fixtures_dir());

    // session-abc has a sessions-index.json entry with a summary -> label
    // must be the summary, not the UUID or firstPrompt.
    assert_eq!(
        session_label("session-abc", &index),
        "Set up a worktree and looked up a CVE"
    );
    assert_eq!(
        session_branch("session-abc", &index),
        Some("feature-branch".to_string())
    );

    // session-xyz's project has no sessions-index.json at all -> falls all
    // the way back to the raw session UUID, and branch lookup is None.
    assert_eq!(session_label("session-xyz", &index), "session-xyz");
    assert_eq!(session_branch("session-xyz", &index), None);
}

#[test]
fn since_filtering_composes_with_the_parsed_invocation_timestamps() {
    let invs = parser::iter_invocations_main_only(&fixtures_dir());
    let since = "2026-02-01T00:00:00Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let after: Vec<_> = invs.iter().filter(|i| i.timestamp >= since).collect();
    // Only the other-project's /terraform (2026-02-10) is on/after this cutoff;
    // demo-project's invocations are all from 2026-01-01.
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].skill_name, "terraform");
}

#[test]
fn load_session_index_is_reusable_across_calls_and_modules() {
    // Smoke check that the public re-export path (`skillscope::sessions`)
    // used by the CLI layer resolves the same way as the direct import used
    // in the rest of this file.
    let index = sessions::load_session_index(&fixtures_dir());
    assert_eq!(index.len(), 1);
}
