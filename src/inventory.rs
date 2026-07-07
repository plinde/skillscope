//! `skillscope inventory` — installed-skill scan joined against invocation
//! history.
//!
//! Answers "when was skill abc last executed, from which session, and how
//! was it triggered?" for every skill actually installed on disk — the
//! inverse join of `summary` (which only shows skills that ever fired).
//! Skills installed but never invoked are the interesting rows here.
//!
//! Discovery reuses the fidelity layer's SKILL.md walk (user roots +
//! plugin marketplaces, deduped by canonical path so the
//! `~/.claude/skills` -> `~/.agents/skills` symlink doesn't double-count).
//! Each entry additionally records the symlink-resolved location and
//! whether any resolution happened.

use crate::fidelity::{self, iter_skill_md_candidates};
use crate::models::{Origin, SkillInvocation, TriggerType};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct InstalledSkill {
    pub name: String,
    pub description: String,
    /// Skill directory as found during the walk (may traverse symlinks).
    pub path: PathBuf,
    /// Fully symlink-resolved skill directory.
    pub resolved_path: PathBuf,
    /// True when `path` and `resolved_path` differ — the skill is reached
    /// through at least one symlink (dir-level or ancestor-level).
    pub symlinked: bool,
    /// "user" (skills roots) or "plugin" (marketplaces).
    pub source: String,
}

/// Scan skill roots for installed skills. `skills_dirs: None` uses the
/// defaults (`~/.agents/skills`, `~/.claude/skills`) plus plugin
/// marketplaces, matching `fidelity::discover_skills`.
pub fn inventory_skills(skills_dirs: Option<&[PathBuf]>) -> Vec<InstalledSkill> {
    let owned_dirs;
    let include_plugins = skills_dirs.is_none();
    let dirs: &[PathBuf] = match skills_dirs {
        Some(d) => d,
        None => {
            owned_dirs = fidelity::default_skills_dirs();
            &owned_dirs
        }
    };

    let mut out = Vec::new();
    for (skill_md, source) in iter_skill_md_candidates(dirs, include_plugins) {
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        let dir = skill_md
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| skill_md.clone());
        let name = fidelity::parse_frontmatter(&content)
            .and_then(|fm| fm.get("name").and_then(|v| v.as_str()).map(String::from))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| {
                dir.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string()
            });
        let description = fidelity::parse_frontmatter(&content)
            .and_then(|fm| {
                fm.get("description")
                    .and_then(|v| v.as_str())
                    .map(String::from)
            })
            .unwrap_or_default();
        let resolved_path = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        let symlinked = resolved_path != dir;
        out.push(InstalledSkill {
            name,
            description,
            path: dir,
            resolved_path,
            symlinked,
            source,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[derive(Debug, Serialize)]
pub struct LastInvocation {
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub project_path: String,
    pub trigger_type: TriggerType,
    pub origin: Origin,
    pub args: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InventoryRow {
    #[serde(flatten)]
    pub skill: InstalledSkill,
    pub total_invocations: usize,
    pub user_slash: usize,
    pub claude_proactive: usize,
    pub subagent: usize,
    pub last: Option<LastInvocation>,
}

/// Join installed skills against the invocation corpus. Every installed
/// skill gets a row; `last` is None for never-invoked skills. Sorted by
/// last-invoked descending, never-invoked last (alphabetical within).
pub fn join_inventory(skills: Vec<InstalledSkill>, invs: &[SkillInvocation]) -> Vec<InventoryRow> {
    // skill name -> (counts, most recent invocation)
    struct Acc<'a> {
        total: usize,
        user_slash: usize,
        claude_proactive: usize,
        subagent: usize,
        last: &'a SkillInvocation,
    }
    let mut by_name: BTreeMap<&str, Acc> = BTreeMap::new();
    for inv in invs {
        let acc = by_name.entry(inv.skill_name.as_str()).or_insert(Acc {
            total: 0,
            user_slash: 0,
            claude_proactive: 0,
            subagent: 0,
            last: inv,
        });
        acc.total += 1;
        match inv.trigger_type {
            TriggerType::UserSlash => acc.user_slash += 1,
            TriggerType::ClaudeProactive => acc.claude_proactive += 1,
        }
        if inv.origin == Origin::Subagent {
            acc.subagent += 1;
        }
        if inv.timestamp > acc.last.timestamp {
            acc.last = inv;
        }
    }

    let mut rows: Vec<InventoryRow> = skills
        .into_iter()
        .map(|skill| {
            let acc = by_name.get(skill.name.as_str());
            InventoryRow {
                total_invocations: acc.map_or(0, |a| a.total),
                user_slash: acc.map_or(0, |a| a.user_slash),
                claude_proactive: acc.map_or(0, |a| a.claude_proactive),
                subagent: acc.map_or(0, |a| a.subagent),
                last: acc.map(|a| LastInvocation {
                    timestamp: a.last.timestamp,
                    session_id: a.last.session_id.clone(),
                    project_path: a.last.project_path.clone(),
                    trigger_type: a.last.trigger_type,
                    origin: a.last.origin,
                    args: a.last.args.clone(),
                }),
                skill,
            }
        })
        .collect();

    rows.sort_by(|a, b| match (&b.last, &a.last) {
        (Some(bl), Some(al)) => bl.timestamp.cmp(&al.timestamp),
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, Some(_)) => std::cmp::Ordering::Less,
        (None, None) => a.skill.name.cmp(&b.skill.name),
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::fs;
    use tempfile::TempDir;

    fn write_skill(root: &std::path::Path, dir_name: &str, frontmatter_name: Option<&str>) {
        let dir = root.join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        let name_line = frontmatter_name
            .map(|n| format!("name: {n}\n"))
            .unwrap_or_default();
        fs::write(
            dir.join("SKILL.md"),
            format!("---\n{name_line}description: test skill\n---\nbody\n"),
        )
        .unwrap();
    }

    fn inv(skill: &str, day: u32, trigger: TriggerType, origin: Origin) -> SkillInvocation {
        SkillInvocation {
            skill_name: skill.to_string(),
            trigger_type: trigger,
            session_id: format!("sess-{day}"),
            project_path: "/repo".to_string(),
            timestamp: Utc.with_ymd_and_hms(2026, 7, day, 12, 0, 0).unwrap(),
            transcript_file: "/tmp/t.jsonl".to_string(),
            args: None,
            origin,
        }
    }

    #[test]
    fn inventory_scans_dirs_and_prefers_frontmatter_name() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "alpha", None);
        write_skill(tmp.path(), "beta-dir", Some("beta"));
        let skills = inventory_skills(Some(&[tmp.path().to_path_buf()]));
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert!(skills.iter().all(|s| s.source == "user"));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skill_dirs_are_detected_and_deduped() {
        let tmp = TempDir::new().unwrap();
        // Canonicalize the tmp root first: on macOS TempDir lives under
        // /var -> /private/var, which would otherwise flag every entry.
        let tmp_root = tmp.path().canonicalize().unwrap();
        let real_root = tmp_root.join("agents-skills");
        write_skill(&real_root, "gamma", None);
        // Mirror the ~/.claude/skills -> ~/.agents/skills convention.
        let link_root = tmp_root.join("claude-skills");
        std::os::unix::fs::symlink(&real_root, &link_root).unwrap();

        // Both roots configured: the canonical dedup must yield ONE gamma,
        // found via the first (real) root, so not marked symlinked.
        let skills = inventory_skills(Some(&[real_root.clone(), link_root.clone()]));
        assert_eq!(skills.len(), 1);
        assert!(!skills[0].symlinked);

        // Only the symlinked root configured: still one gamma, now reached
        // through the link — flagged, with resolved_path under the real root.
        let via_link = inventory_skills(Some(&[link_root]));
        assert_eq!(via_link.len(), 1);
        assert!(via_link[0].symlinked);
        let canon_real = real_root.canonicalize().unwrap();
        assert!(via_link[0].resolved_path.starts_with(&canon_real));
    }

    #[test]
    fn join_reports_last_invocation_with_trigger_context() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "alpha", None);
        write_skill(tmp.path(), "never-used", None);
        let skills = inventory_skills(Some(&[tmp.path().to_path_buf()]));

        let invs = vec![
            inv("alpha", 1, TriggerType::UserSlash, Origin::Main),
            inv("alpha", 3, TriggerType::ClaudeProactive, Origin::Subagent),
            inv("alpha", 2, TriggerType::UserSlash, Origin::Main),
        ];
        let rows = join_inventory(skills, &invs);

        // Invoked skills sort before never-invoked ones.
        assert_eq!(rows[0].skill.name, "alpha");
        assert_eq!(rows[0].total_invocations, 3);
        assert_eq!(rows[0].user_slash, 2);
        assert_eq!(rows[0].claude_proactive, 1);
        assert_eq!(rows[0].subagent, 1);
        let last = rows[0].last.as_ref().unwrap();
        assert_eq!(last.session_id, "sess-3");
        assert_eq!(last.trigger_type, TriggerType::ClaudeProactive);
        assert_eq!(last.origin, Origin::Subagent);

        assert_eq!(rows[1].skill.name, "never-used");
        assert_eq!(rows[1].total_invocations, 0);
        assert!(rows[1].last.is_none());
    }

    #[test]
    fn never_invoked_skills_sort_last_alphabetically() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "zz-unused", None);
        write_skill(tmp.path(), "aa-unused", None);
        write_skill(tmp.path(), "used", None);
        let skills = inventory_skills(Some(&[tmp.path().to_path_buf()]));
        let invs = vec![inv("used", 1, TriggerType::UserSlash, Origin::Main)];
        let rows = join_inventory(skills, &invs);
        let names: Vec<&str> = rows.iter().map(|r| r.skill.name.as_str()).collect();
        assert_eq!(names, vec!["used", "aa-unused", "zz-unused"]);
    }

    #[test]
    fn skill_md_without_frontmatter_is_skipped_gracefully() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("no-frontmatter");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "just a plain markdown body\n").unwrap();
        // Directory-name fallback still yields an entry — installed is
        // installed, even with a malformed SKILL.md.
        let skills = inventory_skills(Some(&[tmp.path().to_path_buf()]));
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "no-frontmatter");
        assert!(skills[0].description.is_empty());
    }
}
