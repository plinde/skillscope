//! Saved session rundowns.
//!
//! A rundown is a written summary of one session's skill usage (usually the
//! answer an agent gave to "what skills did this session invoke?") stored
//! together with a snapshot of that session's normalized invocation records.
//!
//! Rundowns live under the XDG data directory, one JSON file per save:
//! `<data>/rundowns/<session-id>/<saved-at>.json`. The snapshot is the part
//! that matters long-term: Claude Code deletes transcripts after
//! `cleanupPeriodDays` (default 30), and a saved rundown keeps the session's
//! invocations searchable after its transcript is gone.

use crate::models::{Origin, SkillInvocation, TriggerCounts};
use crate::resolve::{MIN_PREFIX_LEN, is_full_uuid, is_hex_prefix};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

/// Bumped when a field changes meaning or is removed. Adding an optional
/// field does not bump it.
pub const SCHEMA_VERSION: u32 = 1;

/// Longest snippet `search` returns for one match.
const SNIPPET_CHARS: usize = 160;

/// The skillscope data directory.
///
/// Precedence: `SKILLSCOPE_DATA_DIR`, then `$XDG_DATA_HOME/skillscope`, then
/// `$HOME/.local/share/skillscope`. Per the XDG Base Directory spec an empty
/// or relative value is ignored; the same rule applies to the override.
pub fn data_dir_from(
    override_dir: Option<OsString>,
    xdg_data_home: Option<OsString>,
    home: &Path,
) -> PathBuf {
    if let Some(dir) = absolute(override_dir) {
        return dir;
    }
    if let Some(dir) = absolute(xdg_data_home) {
        return dir.join("skillscope");
    }
    home.join(".local").join("share").join("skillscope")
}

fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_absolute().then_some(path)
}

/// `data_dir_from` with the process environment.
pub fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    data_dir_from(
        std::env::var_os("SKILLSCOPE_DATA_DIR"),
        std::env::var_os("XDG_DATA_HOME"),
        &home,
    )
}

pub fn rundowns_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("rundowns")
}

/// Per-skill tally inside a rundown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillTally {
    pub total: usize,
    #[serde(flatten)]
    pub triggers: TriggerCounts,
    pub subagent: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rundown {
    pub schema: u32,
    pub saved_at: DateTime<Utc>,
    pub session_id: String,
    pub harness: String,
    /// The session's working directory, when the transcript records one.
    pub project_path: Option<String>,
    pub transcript_file: String,
    pub first_seen: Option<DateTime<Utc>>,
    pub last_seen: Option<DateTime<Utc>>,
    pub triggers: TriggerCounts,
    pub skills: BTreeMap<String, SkillTally>,
    /// Snapshot of the session's invocations, oldest first.
    pub invocations: Vec<SkillInvocation>,
    /// The written summary (Markdown), when one was supplied.
    pub summary: Option<String>,
    /// Free-form tag naming who wrote the summary.
    pub author: Option<String>,
    pub skillscope_version: String,
}

impl Rundown {
    /// Build a rundown from a session's invocations. The session's project
    /// path comes from its first invocation, else from `fallback_cwd`.
    pub fn build(
        session_id: &str,
        transcript_file: &Path,
        mut invocations: Vec<SkillInvocation>,
        fallback_cwd: Option<String>,
        summary: Option<String>,
        author: Option<String>,
        saved_at: DateTime<Utc>,
    ) -> Rundown {
        invocations.sort_by_key(|inv| inv.timestamp);
        let mut triggers = TriggerCounts::default();
        let mut skills: BTreeMap<String, SkillTally> = BTreeMap::new();
        for inv in &invocations {
            triggers.record(inv.trigger_type);
            let tally = skills.entry(inv.skill_name.clone()).or_default();
            tally.total += 1;
            tally.triggers.record(inv.trigger_type);
            if inv.origin == Origin::Subagent {
                tally.subagent += 1;
            }
        }
        let summary = summary.filter(|s| !s.trim().is_empty());
        Rundown {
            schema: SCHEMA_VERSION,
            saved_at,
            session_id: session_id.to_string(),
            harness: "claude".to_string(),
            project_path: invocations
                .first()
                .map(|inv| inv.project_path.clone())
                .or(fallback_cwd),
            transcript_file: transcript_file.to_string_lossy().to_string(),
            first_seen: invocations.first().map(|inv| inv.timestamp),
            last_seen: invocations.last().map(|inv| inv.timestamp),
            triggers,
            skills,
            invocations,
            summary,
            author,
            skillscope_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// First non-empty summary line, with leading Markdown heading marks removed.
    pub fn headline(&self) -> Option<String> {
        self.summary.as_deref().and_then(|s| {
            s.lines()
                .map(|l| l.trim().trim_start_matches('#').trim())
                .find(|l| !l.is_empty())
                .map(str::to_string)
        })
    }

    /// Where the query (already lowercased) matches, as a short snippet:
    /// the summary first, then skill names, skill args, the project path.
    pub fn find(&self, query_lower: &str) -> Option<String> {
        if let Some(summary) = &self.summary
            && let Some(line) = summary
                .lines()
                .find(|l| l.to_lowercase().contains(query_lower))
        {
            return Some(truncate(line.trim(), SNIPPET_CHARS));
        }
        if let Some(name) = self
            .skills
            .keys()
            .find(|k| k.to_lowercase().contains(query_lower))
        {
            return Some(format!("skill: {name}"));
        }
        if let Some(inv) = self.invocations.iter().find(|inv| {
            inv.args
                .as_deref()
                .is_some_and(|a| a.to_lowercase().contains(query_lower))
        }) {
            let args = inv.args.as_deref().unwrap_or_default();
            return Some(truncate(
                &format!("{} args: {}", inv.skill_name, args.replace('\n', " ")),
                SNIPPET_CHARS,
            ));
        }
        if let Some(project) = &self.project_path
            && project.to_lowercase().contains(query_lower)
        {
            return Some(format!("project: {project}"));
        }
        None
    }
}

/// Truncate to at most `max` characters, marking the cut with `…`.
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The `cwd` recorded on the first transcript line that has one.
pub fn transcript_cwd(transcript: &Path) -> Option<String> {
    let file = fs::File::open(transcript).ok()?;
    for line in io::BufReader::new(file).lines().map_while(Result::ok) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&line)
            && let Some(cwd) = value.get("cwd").and_then(|v| v.as_str())
        {
            return Some(cwd.to_string());
        }
    }
    None
}

fn file_name_for(saved_at: DateTime<Utc>) -> String {
    format!("{}.json", saved_at.format("%Y%m%dT%H%M%S%.3fZ"))
}

/// Write a rundown under `rundowns_dir/<session-id>/`, atomically (temp file
/// plus rename). Directories are created 0700 and files 0600 on Unix: a
/// summary can quote anything that was in the session.
pub fn save(rundowns_dir: &Path, rundown: &Rundown) -> io::Result<PathBuf> {
    let dir = rundowns_dir.join(&rundown.session_id);
    create_private_dir(&dir)?;
    let name = file_name_for(rundown.saved_at);
    let path = dir.join(&name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let body = serde_json::to_string_pretty(rundown).map_err(io::Error::other)?;
    {
        let mut file = open_private(&tmp)?;
        file.write_all(body.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

#[cfg(unix)]
fn open_private(path: &Path) -> io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
}

pub fn load(path: &Path) -> Result<Rundown, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every saved rundown, oldest first. Unreadable files are reported on
/// stderr and skipped.
pub fn load_all(rundowns_dir: &Path) -> Vec<(PathBuf, Rundown)> {
    let mut out = Vec::new();
    let Ok(sessions) = fs::read_dir(rundowns_dir) else {
        return out;
    };
    for session in sessions.flatten() {
        let Ok(files) = fs::read_dir(session.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            let is_rundown = path.extension().is_some_and(|e| e == "json")
                && !path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with('.'));
            if !is_rundown {
                continue;
            }
            match load(&path) {
                Ok(rundown) => out.push((path, rundown)),
                Err(e) => eprintln!("skillscope: skipping unreadable rundown {e}"),
            }
        }
    }
    out.sort_by(|a, b| a.1.saved_at.cmp(&b.1.saved_at).then(a.0.cmp(&b.0)));
    out
}

/// The newest rundown for `target`: a path to a rundown file, a full
/// session UUID, or a >=8-char hex prefix that names exactly one session.
pub fn latest_for(all: &[(PathBuf, Rundown)], target: &str) -> Result<(PathBuf, Rundown), String> {
    let as_path = Path::new(target);
    if target.ends_with(".json") && as_path.is_file() {
        return load(as_path).map(|r| (as_path.to_path_buf(), r));
    }
    let wanted = target.to_lowercase();
    let exact = is_full_uuid(&wanted);
    if !exact {
        if !is_hex_prefix(&wanted) {
            return Err(format!(
                "'{target}' is not a session id or rundown file: expected a full UUID, a >=8-char hex prefix, or a path to a .json rundown"
            ));
        }
        if wanted.len() < MIN_PREFIX_LEN {
            return Err(format!(
                "session-id prefix '{target}' is too short: need at least {MIN_PREFIX_LEN} hex chars"
            ));
        }
    }
    let hits: Vec<&(PathBuf, Rundown)> = all
        .iter()
        .filter(|(_, r)| {
            let id = r.session_id.to_lowercase();
            if exact {
                id == wanted
            } else {
                id.starts_with(&wanted)
            }
        })
        .collect();
    let mut ids: Vec<&str> = hits.iter().map(|(_, r)| r.session_id.as_str()).collect();
    ids.sort();
    ids.dedup();
    match ids.len() {
        0 => Err(format!("no saved rundown for session '{target}'")),
        1 => Ok((*hits.last().expect("non-empty")).clone()),
        _ => Err(format!(
            "ambiguous session-id prefix '{target}'; candidates:\n  {}",
            ids.join("\n  ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Harness, TriggerType};
    use chrono::TimeZone;
    use tempfile::TempDir;

    const SID: &str = "6c029127-e145-45aa-970b-b93f805bd280";
    const SID_SIBLING: &str = "6c029127-ffff-45aa-970b-b93f805bd280";

    fn at(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 11, h, m, 0).unwrap()
    }

    fn inv(skill: &str, trigger: TriggerType, origin: Origin, t: DateTime<Utc>) -> SkillInvocation {
        SkillInvocation {
            skill_name: skill.to_string(),
            trigger_type: trigger,
            session_id: SID.to_string(),
            project_path: "/work/demo".to_string(),
            timestamp: t,
            transcript_file: "/t.jsonl".to_string(),
            args: Some(format!("args for {skill}")),
            origin,
            harness: Harness::Claude,
        }
    }

    fn sample(session: &str, saved: DateTime<Utc>, summary: Option<&str>) -> Rundown {
        Rundown::build(
            session,
            Path::new("/t.jsonl"),
            vec![
                inv(
                    "todo",
                    TriggerType::ClaudeProactive,
                    Origin::Main,
                    at(10, 5),
                ),
                inv("rtfm", TriggerType::UserSlash, Origin::Main, at(10, 1)),
                inv(
                    "rtfm",
                    TriggerType::ClaudeProactive,
                    Origin::Subagent,
                    at(10, 3),
                ),
            ],
            None,
            summary.map(str::to_string),
            Some("tester".to_string()),
            saved,
        )
    }

    #[test]
    fn data_dir_precedence_follows_override_then_xdg_then_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            data_dir_from(Some("/override".into()), Some("/xdg".into()), home),
            PathBuf::from("/override")
        );
        assert_eq!(
            data_dir_from(None, Some("/xdg".into()), home),
            PathBuf::from("/xdg/skillscope")
        );
        assert_eq!(
            data_dir_from(None, None, home),
            PathBuf::from("/home/u/.local/share/skillscope")
        );
    }

    #[test]
    fn empty_or_relative_values_are_ignored() {
        let home = Path::new("/home/u");
        let default = PathBuf::from("/home/u/.local/share/skillscope");
        assert_eq!(
            data_dir_from(Some("".into()), Some("".into()), home),
            default
        );
        assert_eq!(
            data_dir_from(Some("rel/dir".into()), Some("xdg-rel".into()), home),
            default
        );
    }

    #[test]
    fn build_sorts_invocations_and_tallies_triggers() {
        let r = sample(SID, at(12, 0), Some("## Rundown\nbody"));
        let order: Vec<_> = r.invocations.iter().map(|i| i.timestamp).collect();
        assert_eq!(order, vec![at(10, 1), at(10, 3), at(10, 5)]);
        assert_eq!(r.first_seen, Some(at(10, 1)));
        assert_eq!(r.last_seen, Some(at(10, 5)));
        assert_eq!(r.triggers.user_slash, 1);
        assert_eq!(r.triggers.claude_proactive, 2);
        let rtfm = &r.skills["rtfm"];
        assert_eq!((rtfm.total, rtfm.subagent), (2, 1));
        assert_eq!(r.project_path.as_deref(), Some("/work/demo"));
        assert_eq!(r.headline().as_deref(), Some("Rundown"));
    }

    #[test]
    fn blank_summary_is_stored_as_none() {
        let r = sample(SID, at(12, 0), Some("  \n "));
        assert!(r.summary.is_none());
        assert!(r.headline().is_none());
    }

    #[test]
    fn empty_session_uses_fallback_cwd() {
        let r = Rundown::build(
            SID,
            Path::new("/t.jsonl"),
            vec![],
            Some("/from/transcript".to_string()),
            None,
            None,
            at(12, 0),
        );
        assert_eq!(r.project_path.as_deref(), Some("/from/transcript"));
        assert!(r.first_seen.is_none());
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = TempDir::new().unwrap();
        let r = sample(SID, at(12, 0), Some("summary text"));
        let path = save(tmp.path(), &r).unwrap();
        assert_eq!(path, tmp.path().join(SID).join("20261111T120000.000Z.json"));
        let back = load(&path).unwrap();
        assert_eq!(back.session_id, SID);
        assert_eq!(back.invocations.len(), 3);
        assert_eq!(back.skills, r.skills);
        assert_eq!(back.summary.as_deref(), Some("summary text"));
        // no temp file left behind
        let leftovers: Vec<_> = fs::read_dir(tmp.path().join(SID))
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn saved_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let path = save(tmp.path(), &sample(SID, at(12, 0), None)).unwrap();
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        let dir_mode = fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600);
        assert_eq!(dir_mode, 0o700);
    }

    #[test]
    fn latest_for_picks_newest_save_of_one_session() {
        let tmp = TempDir::new().unwrap();
        save(tmp.path(), &sample(SID, at(12, 0), Some("old"))).unwrap();
        save(tmp.path(), &sample(SID, at(13, 0), Some("new"))).unwrap();
        let all = load_all(tmp.path());
        assert_eq!(all.len(), 2);
        let (_, r) = latest_for(&all, "6c029127").unwrap();
        assert_eq!(r.summary.as_deref(), Some("new"));
    }

    #[test]
    fn latest_for_rejects_ambiguous_short_and_bad_targets() {
        let tmp = TempDir::new().unwrap();
        save(tmp.path(), &sample(SID, at(12, 0), None)).unwrap();
        save(tmp.path(), &sample(SID_SIBLING, at(12, 1), None)).unwrap();
        let all = load_all(tmp.path());
        let err = latest_for(&all, "6c029127").unwrap_err();
        assert!(err.contains("ambiguous") && err.contains(SID_SIBLING));
        assert!(latest_for(&all, "6c02").unwrap_err().contains("too short"));
        assert!(latest_for(&all, "not-a-session").is_err());
        assert!(
            latest_for(&all, "99999999-9999-4999-8999-999999999999")
                .unwrap_err()
                .contains("no saved rundown")
        );
        assert_eq!(latest_for(&all, SID).unwrap().1.session_id, SID);
    }

    #[test]
    fn latest_for_accepts_a_rundown_file_path() {
        let tmp = TempDir::new().unwrap();
        let path = save(tmp.path(), &sample(SID, at(12, 0), None)).unwrap();
        let (got, r) = latest_for(&[], path.to_str().unwrap()).unwrap();
        assert_eq!(got, path);
        assert_eq!(r.session_id, SID);
    }

    #[test]
    fn load_all_skips_temp_and_unreadable_files() {
        let tmp = TempDir::new().unwrap();
        save(tmp.path(), &sample(SID, at(12, 0), None)).unwrap();
        let dir = tmp.path().join(SID);
        fs::write(dir.join(".x.json.tmp"), "{").unwrap();
        fs::write(dir.join(".hidden.json"), "{").unwrap();
        fs::write(dir.join("broken.json"), "{").unwrap();
        assert_eq!(load_all(tmp.path()).len(), 1);
        assert!(load_all(&tmp.path().join("missing")).is_empty());
    }

    #[test]
    fn find_prefers_summary_then_skill_then_args_then_project() {
        let r = sample(SID, at(12, 0), Some("Line one\nThe Deadlock was fixed"));
        assert_eq!(
            r.find("deadlock").as_deref(),
            Some("The Deadlock was fixed")
        );
        assert_eq!(r.find("rtf").as_deref(), Some("skill: rtfm"));
        assert_eq!(
            r.find("args for todo").as_deref(),
            Some("todo args: args for todo")
        );
        assert_eq!(r.find("work/demo").as_deref(), Some("project: /work/demo"));
        assert!(r.find("nowhere").is_none());
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("héllo", 10), "héllo");
        assert_eq!(truncate("héllo wörld", 5), "héll…");
    }
}
