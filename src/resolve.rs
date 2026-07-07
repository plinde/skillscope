//! Session-id resolution for `skillscope <session-id>`.
//!
//! A transcript's filename stem IS its session id, so resolution is a walk
//! over `<projects_dir>/*/*.jsonl` matching stems — exact for a full UUID,
//! prefix for a git-style ≥8-hex-char abbreviation.

use crate::parser::glob_main_transcripts;
use std::fmt;
use std::path::{Path, PathBuf};

const MIN_PREFIX_LEN: usize = 8;

#[derive(Debug)]
pub enum ResolveError {
    /// Target isn't hex/UUID-shaped at all.
    InvalidFormat(String),
    /// Hex, but shorter than the 8-char git-style minimum.
    PrefixTooShort(String),
    /// No transcript stem matches.
    NoMatch(String),
    /// Prefix matches more than one distinct session id.
    AmbiguousPrefix(Vec<String>),
    /// One session id, but its transcript exists in multiple project dirs.
    DuplicateAcrossProjects(Vec<PathBuf>),
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveError::InvalidFormat(t) => write!(
                f,
                "'{t}' is not a session id: expected a full UUID or a >=8-char hex prefix"
            ),
            ResolveError::PrefixTooShort(t) => write!(
                f,
                "session-id prefix '{t}' is too short: need at least {MIN_PREFIX_LEN} hex chars"
            ),
            ResolveError::NoMatch(t) => write!(f, "no session found matching '{t}'"),
            ResolveError::AmbiguousPrefix(ids) => {
                writeln!(f, "ambiguous session-id prefix; candidates:")?;
                for id in ids {
                    writeln!(f, "  {id}")?;
                }
                Ok(())
            }
            ResolveError::DuplicateAcrossProjects(paths) => {
                writeln!(f, "session id found in multiple project directories:")?;
                for p in paths {
                    writeln!(f, "  {}", p.display())?;
                }
                Ok(())
            }
        }
    }
}

/// Full session UUID: 8-4-4-4-12 lowercase/uppercase hex groups.
fn is_full_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&parts)
            .all(|(len, part)| part.len() == *len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn is_hex_prefix(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Resolve a full session UUID or a ≥8-hex-char prefix to its main
/// transcript path under `projects_dir`.
pub fn resolve_session_id(
    projects_dir: &Path,
    id_or_prefix: &str,
) -> Result<PathBuf, ResolveError> {
    let target = id_or_prefix.to_lowercase();
    let exact = if is_full_uuid(&target) {
        true
    } else if is_hex_prefix(&target) {
        if target.len() < MIN_PREFIX_LEN {
            return Err(ResolveError::PrefixTooShort(id_or_prefix.to_string()));
        }
        false
    } else {
        return Err(ResolveError::InvalidFormat(id_or_prefix.to_string()));
    };

    let mut matches: Vec<(String, PathBuf)> = Vec::new();
    for path in glob_main_transcripts(projects_dir) {
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let stem_lower = stem.to_lowercase();
        let hit = if exact {
            stem_lower == target
        } else {
            stem_lower.starts_with(&target)
        };
        if hit {
            matches.push((stem_lower, path));
        }
    }

    if matches.is_empty() {
        return Err(ResolveError::NoMatch(id_or_prefix.to_string()));
    }

    let mut stems: Vec<String> = matches.iter().map(|(s, _)| s.clone()).collect();
    stems.sort();
    stems.dedup();
    if stems.len() > 1 {
        return Err(ResolveError::AmbiguousPrefix(stems));
    }
    if matches.len() > 1 {
        let mut paths: Vec<PathBuf> = matches.into_iter().map(|(_, p)| p).collect();
        paths.sort();
        return Err(ResolveError::DuplicateAcrossProjects(paths));
    }
    Ok(matches.remove(0).1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    const UUID_A: &str = "6c029127-e145-45aa-970b-b93f805bd280";
    const UUID_A_SIBLING: &str = "6c029127-ffff-45aa-970b-b93f805bd280"; // shares 8-char prefix
    const UUID_B: &str = "ac1d416d-1762-487f-a68f-1725c5e8e64e";

    fn touch_transcript(projects_dir: &Path, project: &str, stem: &str) -> PathBuf {
        let dir = projects_dir.join(project);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{stem}.jsonl"));
        fs::write(&path, "").unwrap();
        path
    }

    #[test]
    fn full_uuid_resolves_exactly() {
        let tmp = TempDir::new().unwrap();
        let expected = touch_transcript(tmp.path(), "-proj-a", UUID_A);
        touch_transcript(tmp.path(), "-proj-a", UUID_B);
        let resolved = resolve_session_id(tmp.path(), UUID_A).unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn unique_eight_char_prefix_resolves() {
        let tmp = TempDir::new().unwrap();
        let expected = touch_transcript(tmp.path(), "-proj-a", UUID_B);
        touch_transcript(tmp.path(), "-proj-a", UUID_A);
        let resolved = resolve_session_id(tmp.path(), "ac1d416d").unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn prefix_shorter_than_eight_is_rejected() {
        let tmp = TempDir::new().unwrap();
        touch_transcript(tmp.path(), "-proj-a", UUID_A);
        assert!(matches!(
            resolve_session_id(tmp.path(), "6c02912"),
            Err(ResolveError::PrefixTooShort(_))
        ));
    }

    #[test]
    fn non_hex_target_is_invalid_format() {
        let tmp = TempDir::new().unwrap();
        assert!(matches!(
            resolve_session_id(tmp.path(), "not-a-session"),
            Err(ResolveError::InvalidFormat(_))
        ));
        assert!(matches!(
            resolve_session_id(tmp.path(), "zzzzzzzz"),
            Err(ResolveError::InvalidFormat(_))
        ));
    }

    #[test]
    fn ambiguous_prefix_lists_sorted_candidate_ids() {
        let tmp = TempDir::new().unwrap();
        touch_transcript(tmp.path(), "-proj-a", UUID_A);
        touch_transcript(tmp.path(), "-proj-b", UUID_A_SIBLING);
        match resolve_session_id(tmp.path(), "6c029127") {
            Err(ResolveError::AmbiguousPrefix(ids)) => {
                assert_eq!(ids, vec![UUID_A.to_string(), UUID_A_SIBLING.to_string()]);
            }
            other => panic!("expected AmbiguousPrefix, got {other:?}"),
        }
    }

    #[test]
    fn same_id_across_project_dirs_lists_sorted_paths() {
        let tmp = TempDir::new().unwrap();
        let p1 = touch_transcript(tmp.path(), "-proj-a", UUID_A);
        let p2 = touch_transcript(tmp.path(), "-proj-b", UUID_A);
        match resolve_session_id(tmp.path(), UUID_A) {
            Err(ResolveError::DuplicateAcrossProjects(paths)) => {
                let mut expected = vec![p1, p2];
                expected.sort();
                assert_eq!(paths, expected);
            }
            other => panic!("expected DuplicateAcrossProjects, got {other:?}"),
        }
    }

    #[test]
    fn no_match_is_reported() {
        let tmp = TempDir::new().unwrap();
        touch_transcript(tmp.path(), "-proj-a", UUID_A);
        assert!(matches!(
            resolve_session_id(tmp.path(), UUID_B),
            Err(ResolveError::NoMatch(_))
        ));
    }

    #[test]
    fn resolution_is_case_insensitive() {
        let tmp = TempDir::new().unwrap();
        let expected = touch_transcript(tmp.path(), "-proj-a", UUID_A);
        let upper = UUID_A.to_uppercase();
        assert_eq!(resolve_session_id(tmp.path(), &upper).unwrap(), expected);
        assert_eq!(
            resolve_session_id(tmp.path(), "6C029127").unwrap(),
            expected
        );
    }
}
