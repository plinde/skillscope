//! Static walk of an installed skill's directory: every bundled file, its
//! size, whether it has a table of contents, what it links to, and how deep
//! it sits below SKILL.md. Feeds `skillscope lint` and the static columns
//! of `skillscope refs`.
//!
//! Rules (docs = Claude skill best practices):
//! - `nested-ref`: a reference file links another reference file
//!   (docs `#avoid-deeply-nested-references`).
//! - `ref-over-100`: a reference file over 100 lines; split it by domain
//!   (local policy, docs `#pattern-2-domain-specific-organization`). With a
//!   table of contents it is `ref-over-100-toc`, lower severity (docs
//!   `#structure-longer-reference-files-with-table-of-contents`).
//! - `orphan-file`: a bundled file nothing reachable from SKILL.md names
//!   (docs `#observe-how-claude-navigates-skills`, "Missed connections").
//! - `body-over-500`: SKILL.md body over 500 lines (docs `#token-budgets`).

use once_cell::sync::Lazy;
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Component, Path, PathBuf};

pub const REF_LINE_LIMIT: usize = 100;
pub const BODY_LINE_LIMIT: usize = 500;
/// A table of contents counts only near the top of the file.
const TOC_WINDOW: usize = 30;
/// Directories that hold tooling, not skill content.
/// Directories that hold tooling or test harnesses, not content for the
/// model.
const SKIP_DIRS: &[&str] = &["node_modules", "__pycache__", "target", "venv", "evals"];
/// Files the harness reads itself (Codex skill metadata), not the model.
const HARNESS_FILES: &[&str] = &["agents/openai.yaml"];

static MD_LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[[^\]]*\]\(([^)\s]+)\)").unwrap());
static BACKTICK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`\s]+)`").unwrap());
static PATH_TOKEN_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:[A-Za-z0-9_.-]+/)*[A-Za-z0-9_-][A-Za-z0-9_.-]*\.[A-Za-z0-9]+").unwrap()
});
/// A directory path (`assets/fixtures/`): everything under it is reachable.
static DIR_TOKEN_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:[A-Za-z0-9_.-]+/)+[A-Za-z0-9_.-]*").unwrap());
static TOC_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^#{1,6}\s*(table of contents|contents)\s*$").unwrap());

#[derive(Debug, Clone, Serialize)]
pub struct BundledFile {
    /// Path relative to the skill directory; `SKILL.md` for the entry point.
    pub rel_path: String,
    pub lines: usize,
    pub has_toc: bool,
    /// Steps from SKILL.md (0 = SKILL.md, 1 = named by it); None = orphan.
    pub depth: Option<usize>,
    /// Files this one links to outside code fences (markdown links,
    /// backticked names).
    pub links: Vec<String>,
}

impl BundledFile {
    pub fn is_reference(&self) -> bool {
        self.rel_path != "SKILL.md" && self.rel_path.ends_with(".md")
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillTree {
    pub skill: String,
    pub dir: PathBuf,
    /// SKILL.md body lines (after frontmatter).
    pub body_lines: usize,
    /// SKILL.md first, then bundled files sorted by path.
    pub files: Vec<BundledFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Warn,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub skill: String,
    pub code: &'static str,
    pub severity: Severity,
    pub file: String,
    pub detail: String,
    /// Docs anchor or "local policy".
    pub source: &'static str,
}

/// Lexically normalize `base/rel` (no filesystem access, `..` collapsed).
fn join_norm(base: &Path, rel: &str) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in base.join(rel).components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            c => out.push(c),
        }
    }
    out
}

/// Resolve a name written in `from` to a bundled file: relative to the
/// file's directory, then to the skill root, then by unique basename.
fn resolve(name: &str, from: &str, known: &BTreeSet<String>) -> Option<String> {
    let name = name
        .split('#')
        .next()
        .unwrap_or(name)
        .trim_start_matches("./");
    if name.is_empty() || name.contains("://") || name.starts_with('/') || name.starts_with('~') {
        return None;
    }
    let from_dir = Path::new(from).parent().unwrap_or(Path::new(""));
    for cand in [join_norm(from_dir, name), join_norm(Path::new(""), name)] {
        let cand = cand.to_string_lossy().to_string();
        if cand != from && known.contains(&cand) {
            return Some(cand);
        }
    }
    let base = Path::new(name).file_name()?.to_str()?;
    let mut hits = known
        .iter()
        .filter(|k| *k != from && Path::new(k).file_name().and_then(|f| f.to_str()) == Some(base));
    let hit = hits.next()?;
    hits.next().is_none().then(|| hit.clone())
}

/// Every bundled file under the directory `name` names, relative to
/// `from`'s directory or the skill root.
fn files_under(name: &str, from: &str, known: &BTreeSet<String>) -> Vec<String> {
    let name = name.trim_start_matches("./").trim_end_matches('/');
    if name.is_empty() {
        return Vec::new();
    }
    let from_dir = Path::new(from).parent().unwrap_or(Path::new(""));
    for cand in [join_norm(from_dir, name), join_norm(Path::new(""), name)] {
        let prefix = format!("{}/", cand.to_string_lossy());
        let hits: Vec<String> = known
            .iter()
            .filter(|k| k.starts_with(&prefix) && *k != from)
            .cloned()
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    Vec::new()
}

/// (links outside code fences, every path-like mention anywhere).
fn scan_text(
    text: &str,
    from: &str,
    is_md: bool,
    known: &BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let mut links = BTreeSet::new();
    let mut mentions = BTreeSet::new();
    let mut in_fence = false;
    for line in text.lines() {
        if is_md && (line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~")) {
            in_fence = !in_fence;
            continue;
        }
        if is_md && !in_fence {
            for cap in MD_LINK_RE
                .captures_iter(line)
                .chain(BACKTICK_RE.captures_iter(line))
            {
                if let Some(t) = resolve(&cap[1], from, known) {
                    links.insert(t);
                }
            }
        }
        for m in PATH_TOKEN_RE.find_iter(line) {
            if let Some(t) = resolve(m.as_str(), from, known) {
                mentions.insert(t);
            }
        }
        for m in DIR_TOKEN_RE.find_iter(line) {
            mentions.extend(files_under(m.as_str(), from, known));
        }
    }
    (links.into_iter().collect(), mentions.into_iter().collect())
}

fn body_lines(skill_md: &str) -> usize {
    let mut lines = skill_md.lines();
    if skill_md.starts_with("---") {
        lines.next();
        for l in lines.by_ref() {
            if l.trim_end() == "---" {
                break;
            }
        }
    }
    lines.count()
}

/// Walk one skill directory. None when it has no readable SKILL.md.
pub fn walk_skill(skill: &str, dir: &Path) -> Option<SkillTree> {
    let skill_md = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let mut texts: BTreeMap<String, Option<String>> = BTreeMap::new();
    let walker = walkdir::WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let name = e.file_name().to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                return false;
            }
            // A nested skill is its own skill, not bundled content.
            !(e.file_type().is_dir() && e.path().join("SKILL.md").is_file())
        });
    for entry in walker.filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(dir) else {
            continue;
        };
        let rel = rel.to_string_lossy().to_string();
        if rel == "SKILL.md" || rel.starts_with("LICENSE") || HARNESS_FILES.contains(&rel.as_str())
        {
            continue;
        }
        texts.insert(rel, std::fs::read_to_string(entry.path()).ok());
    }
    texts.insert("SKILL.md".to_string(), Some(skill_md.clone()));
    let known: BTreeSet<String> = texts.keys().cloned().collect();

    let mut files: BTreeMap<String, BundledFile> = BTreeMap::new();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (rel, text) in &texts {
        let text = text.as_deref().unwrap_or("");
        let is_md = rel.ends_with(".md");
        let (links, mentions) = scan_text(text, rel, is_md, &known);
        let mut out: BTreeSet<String> = mentions.into_iter().collect();
        out.extend(links.iter().cloned());
        edges.insert(rel.clone(), out.into_iter().collect());
        files.insert(
            rel.clone(),
            BundledFile {
                rel_path: rel.clone(),
                lines: text.lines().count(),
                has_toc: is_md
                    && text
                        .lines()
                        .take(TOC_WINDOW)
                        .any(|l| TOC_RE.is_match(l.trim())),
                depth: None,
                links,
            },
        );
    }

    // Breadth-first depth from SKILL.md.
    let mut queue = VecDeque::from([("SKILL.md".to_string(), 0usize)]);
    while let Some((rel, d)) = queue.pop_front() {
        let Some(f) = files.get_mut(&rel) else {
            continue;
        };
        if f.depth.is_some() {
            continue;
        }
        f.depth = Some(d);
        for next in edges.get(&rel).into_iter().flatten() {
            queue.push_back((next.clone(), d + 1));
        }
    }

    let entry = files.remove("SKILL.md")?;
    let mut ordered = vec![entry];
    ordered.extend(files.into_values());
    Some(SkillTree {
        skill: skill.to_string(),
        dir: dir.to_path_buf(),
        body_lines: body_lines(&skill_md),
        files: ordered,
    })
}

/// Lint findings for one walked skill, most severe first.
pub fn lint(tree: &SkillTree) -> Vec<Finding> {
    let mut out = Vec::new();
    let finding = |code, severity, file: &str, detail: String, source| Finding {
        skill: tree.skill.clone(),
        code,
        severity,
        file: file.to_string(),
        detail,
        source,
    };
    if tree.body_lines > BODY_LINE_LIMIT {
        out.push(finding(
            "body-over-500",
            Severity::Error,
            "SKILL.md",
            format!("{} body lines", tree.body_lines),
            "docs #token-budgets",
        ));
    }
    let is_ref = |rel: &str| {
        tree.files
            .iter()
            .any(|f| f.rel_path == rel && f.is_reference())
    };
    for f in tree.files.iter().filter(|f| f.is_reference()) {
        for target in f.links.iter().filter(|t| is_ref(t)) {
            out.push(finding(
                "nested-ref",
                Severity::Error,
                &f.rel_path,
                format!("{} -> {}", f.rel_path, target),
                "docs #avoid-deeply-nested-references",
            ));
        }
        if f.lines > REF_LINE_LIMIT {
            let (code, severity, source) = if f.has_toc {
                (
                    "ref-over-100-toc",
                    Severity::Warn,
                    "docs #structure-longer-reference-files-with-table-of-contents",
                )
            } else {
                (
                    "ref-over-100",
                    Severity::Error,
                    "local policy (split by domain)",
                )
            };
            out.push(finding(
                code,
                severity,
                &f.rel_path,
                format!("{} ({} lines)", f.rel_path, f.lines),
                source,
            ));
        }
    }
    for f in tree.files.iter().filter(|f| f.depth.is_none()) {
        out.push(finding(
            "orphan-file",
            Severity::Warn,
            &f.rel_path,
            format!("{} not reachable from SKILL.md", f.rel_path),
            "docs #observe-how-claude-navigates-skills",
        ));
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.severity));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn lines(n: usize) -> String {
        (0..n).map(|i| format!("line {i}\n")).collect()
    }

    fn fixture() -> (TempDir, SkillTree) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write(
            root,
            "SKILL.md",
            "---\nname: demo\ndescription: d\n---\nSee [a](references/a.md), `references/big.md`, \
             `references/toc.md`. Fixtures in `assets/fx/`.\n```bash\nscripts/run.sh\n```\n",
        );
        // a.md links b.md in prose (nested) and c.md only inside a fence.
        write(
            root,
            "references/a.md",
            "Next: [b](b.md)\n```\ncat `c.md`\n```\n",
        );
        write(root, "references/b.md", "leaf\n");
        write(root, "references/c.md", "leaf\n");
        write(root, "references/big.md", &lines(120));
        write(
            root,
            "references/toc.md",
            &format!("# T\n\n## Contents\n- x\n{}", lines(120)),
        );
        write(root, "scripts/run.sh", "echo hi\n");
        write(root, "scripts/lonely.sh", "echo nobody calls me\n");
        write(root, "assets/fx/one/a.txt", "x\n");
        write(root, "evals/cases.yaml", "x\n");
        write(root, "agents/openai.yaml", "x\n");
        let tree = walk_skill("demo", root).unwrap();
        (tmp, tree)
    }

    fn file<'a>(tree: &'a SkillTree, rel: &str) -> &'a BundledFile {
        tree.files.iter().find(|f| f.rel_path == rel).unwrap()
    }

    #[test]
    fn depth_toc_and_lines() {
        let (_tmp, tree) = fixture();
        assert_eq!(tree.files[0].rel_path, "SKILL.md");
        assert_eq!(file(&tree, "references/a.md").depth, Some(1));
        assert_eq!(file(&tree, "references/b.md").depth, Some(2));
        // A fenced mention still connects the file.
        assert_eq!(file(&tree, "references/c.md").depth, Some(2));
        assert_eq!(file(&tree, "scripts/run.sh").depth, Some(1));
        assert_eq!(file(&tree, "scripts/lonely.sh").depth, None);
        // A directory mention reaches everything under it.
        assert_eq!(file(&tree, "assets/fx/one/a.txt").depth, Some(1));
        // Test harnesses and harness metadata are not bundled content.
        assert!(
            !tree
                .files
                .iter()
                .any(|f| f.rel_path.starts_with("evals/") || f.rel_path.starts_with("agents/"))
        );
        assert!(file(&tree, "references/toc.md").has_toc);
        assert!(!file(&tree, "references/big.md").has_toc);
        assert_eq!(file(&tree, "references/big.md").lines, 120);
    }

    #[test]
    fn lint_findings() {
        let (_tmp, tree) = fixture();
        let got: Vec<(&str, String)> = lint(&tree)
            .into_iter()
            .map(|f| (f.code, f.detail))
            .collect();
        assert!(got.contains(&("nested-ref", "references/a.md -> references/b.md".into())));
        // The fenced backtick name is not a nested link.
        assert!(!got.iter().any(|(_, d)| d.contains("-> references/c.md")));
        assert!(got.contains(&("ref-over-100", "references/big.md (120 lines)".into())));
        assert!(
            got.iter()
                .any(|(c, d)| *c == "ref-over-100-toc" && d.starts_with("references/toc.md"))
        );
        assert!(got.contains(&(
            "orphan-file",
            "scripts/lonely.sh not reachable from SKILL.md".into()
        )));
        assert!(!got.iter().any(|(c, _)| *c == "body-over-500"));
    }

    #[test]
    fn long_body_is_flagged_and_frontmatter_is_not_counted() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "SKILL.md",
            &format!("---\nname: x\n---\n{}", lines(501)),
        );
        let tree = walk_skill("x", tmp.path()).unwrap();
        assert_eq!(tree.body_lines, 501);
        assert_eq!(lint(&tree)[0].code, "body-over-500");
    }
}
