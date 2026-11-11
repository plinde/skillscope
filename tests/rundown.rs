//! End-to-end tests for `skillscope rundown`, run against the committed
//! `tests/fixtures/scoped/` transcripts with the store in a temp dir
//! (`SKILLSCOPE_DATA_DIR`), so nothing touches the real data directory.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

const SESSION_ONE: &str = "1a2b3c4d-1111-4000-8000-000000000001";
const SESSION_TWO: &str = "1a2b3c4d-2222-4000-8000-000000000002";
const EMPTY_SESSION: &str = "ffffffff-0000-4000-8000-00000000000f";

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scoped")
}

fn run(data: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_skillscope"));
    cmd.env("SKILLSCOPE_DATA_DIR", data)
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .arg("--projects-dir")
        .arg(fixtures())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn skillscope");
    if let Some(text) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
    }
    drop(child.stdin.take());
    child.wait_with_output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn save_with_summary_then_show_list_and_search() {
    let data = TempDir::new().unwrap();
    let saved = run(
        data.path(),
        &[
            "rundown",
            "save",
            SESSION_ONE,
            "--summary-file",
            "-",
            "--author",
            "test",
        ],
        Some("## Worktree session\nThe agent created a scoped branch.\n"),
    );
    assert!(saved.status.success(), "{saved:?}");
    let path = PathBuf::from(stdout(&saved).trim());
    assert!(path.starts_with(data.path().join("rundowns").join(SESSION_ONE)));
    assert!(path.is_file());

    let shown = run(
        data.path(),
        &["--json", "rundown", "show", "1a2b3c4d-1111"],
        None,
    );
    // a dashed prefix is not a hex prefix: rejected with a clear error
    assert!(!shown.status.success());

    let shown = run(
        data.path(),
        &["--json", "rundown", "show", SESSION_ONE],
        None,
    );
    assert!(shown.status.success(), "{shown:?}");
    let record: serde_json::Value = serde_json::from_str(&stdout(&shown)).unwrap();
    assert_eq!(record["session_id"], SESSION_ONE);
    assert_eq!(record["author"], "test");
    assert_eq!(
        record["project_path"],
        "/Users/test/workspace/scoped-project"
    );
    let skills: Vec<&str> = record["invocations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["skill_name"].as_str().unwrap())
        .collect();
    assert!(skills.contains(&"worktree") && skills.contains(&"cve-lookup"));

    let listed = run(data.path(), &["--json", "rundown", "list"], None);
    let lines: Vec<serde_json::Value> = stdout(&listed)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["headline"], "Worktree session");

    let found = run(
        data.path(),
        &["--json", "rundown", "search", "SCOPED BRANCH"],
        None,
    );
    let hit: serde_json::Value = serde_json::from_str(stdout(&found).trim()).unwrap();
    assert_eq!(hit["match"], "The agent created a scoped branch.");

    let none = run(
        data.path(),
        &["--json", "rundown", "search", "no-such-text"],
        None,
    );
    assert!(stdout(&none).trim().is_empty());
}

#[test]
fn skill_filter_and_ambiguous_prefix() {
    let data = TempDir::new().unwrap();
    for id in [SESSION_ONE, SESSION_TWO] {
        let out = run(data.path(), &["rundown", "save", id], None);
        assert!(out.status.success(), "{out:?}");
    }
    let filtered = run(
        data.path(),
        &["--json", "rundown", "list", "--skill", "cve-lookup"],
        None,
    );
    let ids: Vec<String> = stdout(&filtered)
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["session_id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(ids, vec![SESSION_ONE.to_string()]);

    let ambiguous = run(data.path(), &["rundown", "show", "1a2b3c4d"], None);
    assert!(!ambiguous.status.success());
    let err = String::from_utf8_lossy(&ambiguous.stderr);
    assert!(
        err.contains(SESSION_ONE) && err.contains(SESSION_TWO),
        "{err}"
    );
}

#[test]
fn zero_invocation_session_saves_with_transcript_cwd() {
    let data = TempDir::new().unwrap();
    let out = run(
        data.path(),
        &["--json", "rundown", "save", EMPTY_SESSION],
        None,
    );
    assert!(out.status.success(), "{out:?}");
    let saved: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(saved["invocations"], 0);
    assert_eq!(saved["summary"], false);
}

#[test]
fn save_without_session_needs_the_env_var() {
    let data = TempDir::new().unwrap();
    let out = run(data.path(), &["rundown", "save"], None);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("CLAUDE_CODE_SESSION_ID"));
}

#[test]
fn path_honours_the_override() {
    let data = TempDir::new().unwrap();
    let out = run(data.path(), &["rundown", "path"], None);
    assert_eq!(
        stdout(&out).trim(),
        data.path().join("rundowns").to_string_lossy()
    );
}
