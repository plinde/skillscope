//! Trigger-fidelity eval layer: skill discovery + TF-IDF-weighted classification.
//!
//! No LLM calls. Correlates real user prompts against skill frontmatter
//! descriptions to flag two failure modes:
//!
//! - under-triggering: a skill's description plausibly matches a prompt, but
//!   the skill was never invoked in that session
//! - over-triggering: a skill was invoked in a session, but no prompt in that
//!   session plausibly matches its description
//!
//! Feature 1 replaces the Python reference's ">=2 raw keyword hits" rule,
//! which flagged 346/357 skills as under-triggered on this corpus, with
//! TF-IDF-weighted keyword matching: each keyword match contributes its
//! corpus IDF weight (rare keywords count more than common ones, and terms
//! near-universal across skill descriptions are dropped entirely rather
//! than floored to a small positive value), and a prompt matches a skill
//! when the summed weight clears a tuned threshold. Tuned against the live
//! corpus (`tfidf_threshold`/`min_session_count` defaults of 20.0/8) this
//! lands the under-triggered list at 25 skills, within the spec's ~10-30
//! target range, with evidence dominated by genuine skill-relevant prompts.

use crate::models::{SkillDefinition, UserPrompt};
use crate::parser::{iter_invocations_main_only, iter_user_prompts};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn default_skills_dirs() -> Vec<PathBuf> {
    let home = dirs_home();
    vec![
        home.join(".agents").join("skills"),
        home.join(".claude").join("skills"),
    ]
}

fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

fn plugin_marketplaces_dir() -> PathBuf {
    dirs_home()
        .join(".claude")
        .join("plugins")
        .join("marketplaces")
}

static WORD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"[a-z0-9][a-z0-9_-]*").unwrap());
static TRIGGER_SECTION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)triggers?\s+on\s*:").unwrap());
static QUOTED_PHRASE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#""([^"]{2,60})""#).unwrap());

static STOPWORDS: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    [
        // common English function words
        "a", "an", "the", "and", "or", "but", "if", "then", "else", "when", "while", "for", "to",
        "of", "in", "on", "at", "by", "with", "from", "into", "onto", "over", "under", "about",
        "as", "is", "are", "was", "were", "be", "been", "being", "this", "that", "these", "those",
        "it", "its", "you", "your", "yours", "i", "we", "our", "ours", "they", "their", "he",
        "she", "his", "her", "them", "not", "no", "do", "does", "did", "can", "could", "should",
        "would", "will", "shall", "may", "might", "must", "also", "than", "such", "any", "all",
        "some", "each", "every", "other", "another", "more", "most", "much", "many", "only",
        "just", "very", "so", "too", "own", "same", "here", "there", "what", "which", "who",
        "whom", "how", "why", // generic / domain-neutral words called out in the spec
        "use", "uses", "using", "used", "skill", "skills", "tool", "tools", "user", "users",
        "claude", "agent", "agents", "code", "file", "files", "task", "tasks", "want", "wants",
        "need", "needs",
    ]
    .into_iter()
    .collect()
});

/// Summed-IDF-weight threshold a prompt must clear to "plausibly match" a
/// skill via keyword overlap (exact name / quoted trigger phrase matches
/// bypass this entirely). Tuned against the live corpus — see `run_fidelity`
/// doc comment and the verification report for the sweep that picked this
/// value; it lands the under-triggered list in the 10-30 skill range this
/// spec calls for, versus 346/357 for the Python ">=2 raw hits" rule.
///
/// Overridable via `SKILLSCOPE_TFIDF_THRESHOLD` for empirical re-tuning
/// against a live corpus without a recompile (see `make fidelity-sweep`).
///
/// 20.0 was picked by sweeping 2..100 against the live corpus: below ~15 the
/// weighted-keyword path keeps admitting long, topic-drifting prompts (every
/// extra token is a little more summed weight); above ~25 the under-triggered
/// list stops shrinking at all, because what's left is exclusively the
/// exact-name-as-word bypass (spec-mandated, not weighted) — skills whose
/// name is itself a common word (`security`, `aws`, `gh`, `branch`, `report`,
/// `vpn`, `mfa`) match any prompt that happens to use that word normally.
pub fn tfidf_threshold() -> f64 {
    std::env::var("SKILLSCOPE_TFIDF_THRESHOLD")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20.0)
}

/// Minimum session count for a skill to appear in the under-triggered
/// report. Python's fidelity.py uses a fixed `>= 3`; that value was tuned
/// against the crude ">=2 raw keyword hits" matcher and is too permissive
/// once the name-as-common-word bypass is the dominant source of matches
/// (see `tfidf_threshold` doc comment) — those name matches plateau at 51
/// sessions no matter how the keyword threshold moves, so the count cutoff
/// is the second, independent lever needed to land in the spec's ~10-30
/// target range. 8 was picked by the same sweep.
pub fn min_session_count() -> usize {
    std::env::var("SKILLSCOPE_MIN_SESSION_COUNT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8)
}

#[derive(Debug, Clone)]
pub struct FidelityFinding {
    pub skill_name: String,
    pub count: usize,
    pub evidence: String,
}

#[derive(Debug, Clone)]
pub struct FidelityReport {
    pub under_triggered: Vec<FidelityFinding>,
    pub over_triggered: Vec<FidelityFinding>,
}

#[derive(Debug, Clone)]
struct SkillKeywords {
    name_lower: String,
    /// token -> IDF weight
    weighted_words: HashMap<String, f64>,
    phrases: HashSet<String>,
}

pub(crate) fn parse_frontmatter(content: &str) -> Option<serde_yaml::Value> {
    if !content.starts_with("---") {
        return None;
    }
    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return None;
    }
    serde_yaml::from_str::<serde_yaml::Value>(parts[1])
        .ok()
        .filter(|v| v.is_mapping())
}

/// Walk skill roots (then, when `include_plugins`, the live plugin
/// marketplaces) for `*/SKILL.md`, deduping by canonical path so a skill
/// reachable through both `~/.agents/skills` and the `~/.claude/skills`
/// symlink appears once. Callers pass `include_plugins: false` when the
/// roots were given explicitly (tests, --skills-dir) so the scan stays
/// hermetic. Returns `(skill_md_path_as_found, source)` pairs. Shared
/// with `inventory`.
pub(crate) fn iter_skill_md_candidates(
    skills_dirs: &[PathBuf],
    include_plugins: bool,
) -> Vec<(PathBuf, String)> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut candidates = Vec::new();

    for skills_dir in skills_dirs {
        if !skills_dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(skills_dir) else {
            continue;
        };
        let mut sorted_entries: Vec<_> = entries.flatten().collect();
        sorted_entries.sort_by_key(|e| e.path());
        for entry in sorted_entries {
            let skill_md = entry.path().join("SKILL.md");
            if !skill_md.is_file() {
                continue;
            }
            let resolved = skill_md.canonicalize().unwrap_or(skill_md.clone());
            if seen.contains(&resolved) {
                continue;
            }
            seen.insert(resolved);
            candidates.push((skill_md, "user".to_string()));
        }
    }

    let marketplaces_dir = plugin_marketplaces_dir();
    if include_plugins && marketplaces_dir.is_dir() {
        let mut plugin_paths: Vec<PathBuf> = Vec::new();
        // glob "*/*/skills/*/SKILL.md"
        if let Ok(l1) = std::fs::read_dir(&marketplaces_dir) {
            for m in l1.flatten() {
                let mpath = m.path();
                if !mpath.is_dir() {
                    continue;
                }
                if let Ok(l2) = std::fs::read_dir(&mpath) {
                    for plugin in l2.flatten() {
                        let ppath = plugin.path();
                        let skills_dir = ppath.join("skills");
                        if !skills_dir.is_dir() {
                            continue;
                        }
                        if let Ok(l3) = std::fs::read_dir(&skills_dir) {
                            for skill in l3.flatten() {
                                let skill_md = skill.path().join("SKILL.md");
                                if skill_md.is_file() {
                                    plugin_paths.push(skill_md);
                                }
                            }
                        }
                    }
                }
            }
        }
        plugin_paths.sort();
        for skill_md in plugin_paths {
            let resolved = skill_md.canonicalize().unwrap_or(skill_md.clone());
            if seen.contains(&resolved) {
                continue;
            }
            seen.insert(resolved);
            candidates.push((skill_md, "plugin".to_string()));
        }
    }

    candidates
}

/// Scan skill directories for SKILL.md files and parse their frontmatter.
/// Files without valid YAML frontmatter are skipped gracefully.
pub fn discover_skills(skills_dirs: Option<&[PathBuf]>) -> Vec<SkillDefinition> {
    let owned_dirs;
    let include_plugins = skills_dirs.is_none();
    let dirs: &[PathBuf] = match skills_dirs {
        Some(d) => d,
        None => {
            owned_dirs = default_skills_dirs();
            &owned_dirs
        }
    };
    let mut results = Vec::new();

    for (skill_md, source) in iter_skill_md_candidates(dirs, include_plugins) {
        let Ok(content) = std::fs::read_to_string(&skill_md) else {
            continue;
        };
        let Some(frontmatter) = parse_frontmatter(&content) else {
            continue;
        };
        let name = frontmatter
            .get("name")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| {
                skill_md
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string()
            });
        let description = frontmatter
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        results.push(SkillDefinition {
            name,
            description,
            path: skill_md
                .parent()
                .unwrap_or(Path::new(""))
                .to_string_lossy()
                .to_string(),
            source,
        });
    }

    results
}

fn extract_trigger_phrases(description: &str) -> HashSet<String> {
    let Some(m) = TRIGGER_SECTION_RE.find(description) else {
        return HashSet::new();
    };
    let tail = &description[m.end()..];
    QUOTED_PHRASE_RE
        .captures_iter(tail)
        .filter_map(|c| c.get(1))
        .map(|m| m.as_str().trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn tokenize(text_lower: &str) -> Vec<String> {
    WORD_RE
        .find_iter(text_lower)
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Build the raw (unweighted) token set for a skill, used both to build the
/// IDF corpus and as the per-skill keyword set once weights are known.
fn skill_raw_words(skill: &SkillDefinition) -> HashSet<String> {
    let text = format!("{} {}", skill.name, skill.description).to_lowercase();
    tokenize(&text)
        .into_iter()
        .filter(|t| t.len() >= 4 && !STOPWORDS.contains(t.as_str()))
        .collect()
}

/// Compute IDF weight for every keyword across the skill-description corpus:
/// idf(term) = ln(N / df(term)), where N is the number of skill
/// descriptions (documents) and df is how many of them contain the term at
/// least once. Terms appearing in every skill get weight ~0; rare,
/// distinguishing terms get high weight.
fn compute_idf(skills: &[SkillDefinition]) -> HashMap<String, f64> {
    let n = skills.len().max(1) as f64;
    let mut doc_freq: HashMap<String, usize> = HashMap::new();
    for skill in skills {
        for word in skill_raw_words(skill) {
            *doc_freq.entry(word).or_insert(0) += 1;
        }
    }
    // No floor: a term that appears in most skill descriptions is not a
    // distinguishing signal and must contribute ~0, not a small-but-summable
    // amount. Long prompts (100+ tokens, common in this corpus's session-
    // continuation boilerplate) touch dozens of such terms — a floor here
    // let their weights accumulate past any reasonable threshold, which is
    // what drove the first tuning attempt (threshold 6.0, floor 0.1) to 317
    // false-positive "under-triggered" skills instead of the ~10-30 target.
    doc_freq
        .into_iter()
        .map(|(term, df)| {
            let idf = (n / df as f64).ln().max(0.0);
            (term, idf)
        })
        .collect()
}

fn build_skill_keywords(skill: &SkillDefinition, idf: &HashMap<String, f64>) -> SkillKeywords {
    let phrases = extract_trigger_phrases(&skill.description);
    let words = skill_raw_words(skill);
    let weighted_words = words
        .into_iter()
        .map(|w| {
            let weight = *idf.get(&w).unwrap_or(&0.0);
            (w, weight)
        })
        // Drop near-zero-weight terms outright rather than let a very long
        // prompt accumulate them by sheer word count.
        .filter(|(_, weight)| *weight > 0.5)
        .collect();
    SkillKeywords {
        name_lower: skill.name.to_lowercase(),
        weighted_words,
        phrases,
    }
}

fn name_matches(text_lower: &str, name_lower: &str) -> bool {
    if name_lower.is_empty() {
        return false;
    }
    let dash_re = Regex::new(r"[-_]+").unwrap();
    let normalized_name = dash_re.replace_all(name_lower, " ").trim().to_string();
    if normalized_name.is_empty() {
        return false;
    }
    let normalized_text = dash_re.replace_all(text_lower, " ");
    format!(" {normalized_text} ").contains(&format!(" {normalized_name} "))
}

/// A prompt "plausibly matches" a skill when: it contains the skill name as
/// a word, OR it contains a quoted trigger phrase declared in a "Triggers
/// on:" section, OR the summed IDF weight of its keyword overlap with the
/// skill's keyword set clears `TFIDF_THRESHOLD`.
fn prompt_matches_skill(
    text_lower: &str,
    tokens: &HashSet<String>,
    keywords: &SkillKeywords,
) -> bool {
    if name_matches(text_lower, &keywords.name_lower) {
        return true;
    }
    if keywords
        .phrases
        .iter()
        .any(|p| text_lower.contains(p.as_str()))
    {
        return true;
    }
    let weight: f64 = tokens
        .iter()
        .filter_map(|t| keywords.weighted_words.get(t))
        .sum();
    weight >= tfidf_threshold()
}

/// Additional prompt filters beyond the parser-level ones (Feature 1):
/// drop plan-mode injections, teammate-message wrappers, and anything too
/// short to carry real intent signal.
fn is_noise_prompt(text: &str) -> bool {
    let trimmed = text.trim_start();
    if trimmed.starts_with("Implement the following plan:") {
        return true;
    }
    if trimmed.starts_with("<teammate-message") || trimmed.contains("<teammate-message") {
        return true;
    }
    if text.trim().chars().count() < 15 {
        return true;
    }
    false
}

pub fn run_fidelity(projects_dir: &Path, skills_dirs: Option<&[PathBuf]>) -> FidelityReport {
    let skills = discover_skills(skills_dirs);
    let idf = compute_idf(&skills);
    let skill_keywords: HashMap<String, SkillKeywords> = skills
        .iter()
        .map(|s| (s.name.clone(), build_skill_keywords(s, &idf)))
        .collect();

    let mut session_invocations: HashMap<String, HashSet<String>> = HashMap::new();
    for inv in iter_invocations_main_only(projects_dir) {
        session_invocations
            .entry(inv.session_id)
            .or_default()
            .insert(inv.skill_name);
    }

    let mut session_prompts: HashMap<String, Vec<UserPrompt>> = HashMap::new();
    for prompt in iter_user_prompts(projects_dir) {
        if is_noise_prompt(&prompt.text) {
            continue;
        }
        session_prompts
            .entry(prompt.session_id.clone())
            .or_default()
            .push(prompt);
    }

    let mut under_counts: HashMap<String, usize> = HashMap::new();
    let mut under_examples: HashMap<String, Vec<String>> = HashMap::new();
    let mut over_counts: HashMap<String, usize> = HashMap::new();
    let mut over_examples: HashMap<String, Vec<String>> = HashMap::new();

    let mut all_sessions: HashSet<String> = session_invocations.keys().cloned().collect();
    all_sessions.extend(session_prompts.keys().cloned());

    for session_id in &all_sessions {
        let empty_prompts = Vec::new();
        let prompts = session_prompts.get(session_id).unwrap_or(&empty_prompts);
        let empty_invoked = HashSet::new();
        let invoked = session_invocations
            .get(session_id)
            .unwrap_or(&empty_invoked);

        let mut session_matched: HashSet<String> = HashSet::new();
        let mut matched_snippet: HashMap<String, String> = HashMap::new();

        for prompt in prompts {
            let text_lower = prompt.text.to_lowercase();
            let tokens: HashSet<String> = tokenize(&text_lower).into_iter().collect();
            for (skill_name, keywords) in &skill_keywords {
                if session_matched.contains(skill_name) {
                    continue;
                }
                if prompt_matches_skill(&text_lower, &tokens, keywords) {
                    session_matched.insert(skill_name.clone());
                    let snippet: String = prompt.text.chars().take(120).collect();
                    matched_snippet.insert(skill_name.clone(), snippet);
                }
            }
        }

        for skill_name in session_matched.difference(invoked) {
            *under_counts.entry(skill_name.clone()).or_insert(0) += 1;
            let examples = under_examples.entry(skill_name.clone()).or_default();
            if examples.len() < 3
                && let Some(snippet) = matched_snippet.get(skill_name)
            {
                examples.push(snippet.clone());
            }
        }

        for skill_name in invoked.difference(&session_matched) {
            *over_counts.entry(skill_name.clone()).or_insert(0) += 1;
            let examples = over_examples.entry(skill_name.clone()).or_default();
            if examples.len() < 3 {
                let snippet = prompts
                    .first()
                    .map(|p| p.text.chars().take(120).collect::<String>())
                    .unwrap_or_else(|| "(no user prompt text in session)".to_string());
                examples.push(snippet);
            }
        }
    }

    let min_count = min_session_count();
    let mut under_triggered: Vec<FidelityFinding> = under_counts
        .into_iter()
        .filter(|(_, count)| *count >= min_count)
        .map(|(name, count)| FidelityFinding {
            skill_name: name.clone(),
            count,
            evidence: under_examples
                .get(&name)
                .cloned()
                .unwrap_or_default()
                .join(" | "),
        })
        .collect();
    under_triggered.sort_by(|a, b| b.count.cmp(&a.count));

    let mut over_triggered: Vec<FidelityFinding> = over_counts
        .into_iter()
        .map(|(name, count)| FidelityFinding {
            skill_name: name.clone(),
            count,
            evidence: over_examples
                .get(&name)
                .cloned()
                .unwrap_or_default()
                .join(" | "),
        })
        .collect();
    over_triggered.sort_by(|a, b| b.count.cmp(&a.count));

    FidelityReport {
        under_triggered,
        over_triggered,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// `tfidf_threshold()`/`min_session_count()` read process-global env vars;
    /// cargo runs tests in parallel by default, so tests that mutate those
    /// vars must serialize against each other or they'll stomp on one another.
    static ENV_GUARD: Mutex<()> = Mutex::new(());

    fn skill(name: &str, description: &str) -> SkillDefinition {
        SkillDefinition {
            name: name.to_string(),
            description: description.to_string(),
            path: String::new(),
            source: "user".to_string(),
        }
    }

    // -- name_matches ----------------------------------------------------------

    #[test]
    fn name_matches_whole_word_dash_normalized() {
        assert!(name_matches(
            "please run the cve-lookup skill",
            "cve-lookup"
        ));
        // Dash/underscore normalized on both sides.
        assert!(name_matches("please run the cve lookup now", "cve-lookup"));
        assert!(name_matches("run cve_lookup please", "cve-lookup"));
    }

    #[test]
    fn name_matches_rejects_partial_word_overlap() {
        // "aws" must not match inside "awsome" or "paws".
        assert!(!name_matches("that is pawsome", "aws"));
        assert!(!name_matches("look at this awsome thing", "aws"));
        assert!(name_matches("configure aws now", "aws"));
    }

    #[test]
    fn name_matches_empty_name_never_matches() {
        assert!(!name_matches("anything at all", ""));
    }

    // -- extract_trigger_phrases -------------------------------------------------

    #[test]
    fn extracts_quoted_phrases_after_triggers_on_section() {
        let desc = r#"Some general description. Triggers on: "deploy the app", "run migration"."#;
        let phrases = extract_trigger_phrases(desc);
        assert!(phrases.contains("deploy the app"));
        assert!(phrases.contains("run migration"));
        assert_eq!(phrases.len(), 2);
    }

    #[test]
    fn no_triggers_on_section_yields_no_phrases() {
        let desc = r#"Just a description with "a quoted phrase" but no trigger section."#;
        assert!(extract_trigger_phrases(desc).is_empty());
    }

    // -- compute_idf / build_skill_keywords: floor removal ------------------------

    #[test]
    fn near_universal_terms_get_near_zero_idf_and_are_dropped_from_keywords() {
        // "deployment" appears in every skill's description -> IDF ~= ln(1) = 0,
        // must be filtered out of the weighted keyword set entirely (no floor).
        let skills = vec![
            skill("skill-a", "deployment automation for service alpha"),
            skill("skill-b", "deployment automation for service beta"),
            skill("skill-c", "deployment automation for service gamma"),
        ];
        let idf = compute_idf(&skills);
        assert_eq!(*idf.get("deployment").unwrap_or(&0.0), 0.0);

        let keywords = build_skill_keywords(&skills[0], &idf);
        assert!(
            !keywords.weighted_words.contains_key("deployment"),
            "near-universal term must be dropped, not floored"
        );
    }

    #[test]
    fn rare_distinguishing_terms_get_positive_idf_and_are_kept() {
        let skills = vec![
            skill("skill-a", "deployment automation for service alpha"),
            skill("skill-b", "deployment automation for service beta"),
            skill(
                "skill-c",
                "kyverno policy enforcement engine for kubernetes clusters",
            ),
        ];
        let idf = compute_idf(&skills);
        let kyverno_weight = *idf.get("kyverno").unwrap_or(&0.0);
        assert!(
            kyverno_weight > 1.0,
            "rare term should carry real weight, got {kyverno_weight}"
        );

        let keywords = build_skill_keywords(&skills[2], &idf);
        assert!(keywords.weighted_words.contains_key("kyverno"));
    }

    // -- prompt_matches_skill: instant-match rules --------------------------------

    #[test]
    fn instant_match_via_exact_skill_name_bypasses_threshold() {
        let skills = vec![skill("terraform", "Infrastructure as code workflows.")];
        let idf = compute_idf(&skills);
        let keywords = build_skill_keywords(&skills[0], &idf);
        let text = "please help me with terraform today";
        let tokens: HashSet<String> = tokenize(text).into_iter().collect();
        assert!(prompt_matches_skill(text, &tokens, &keywords));
    }

    #[test]
    fn instant_match_via_quoted_trigger_phrase_bypasses_threshold() {
        let skills = vec![skill(
            "release-helper",
            r#"Helps with releases. Triggers on: "cut a release"."#,
        )];
        let idf = compute_idf(&skills);
        let keywords = build_skill_keywords(&skills[0], &idf);
        let text = "can you cut a release for me please";
        let tokens: HashSet<String> = tokenize(text).into_iter().collect();
        assert!(prompt_matches_skill(text, &tokens, &keywords));
    }

    #[test]
    fn no_match_when_neither_name_phrase_nor_threshold_condition_holds() {
        let skills = vec![skill(
            "unrelated-skill",
            "Something about widgets and gadgets only.",
        )];
        let idf = compute_idf(&skills);
        let keywords = build_skill_keywords(&skills[0], &idf);
        let text = "totally different topic about lunch plans today";
        let tokens: HashSet<String> = tokenize(text).into_iter().collect();
        assert!(!prompt_matches_skill(text, &tokens, &keywords));
    }

    // -- prompt_matches_skill: weighted threshold behavior ------------------------

    #[test]
    fn weighted_path_matches_above_threshold_and_not_below() {
        let _guard = ENV_GUARD.lock().unwrap();
        // Build a corpus where "kyverno" and "policy" are rare (high IDF) and
        // distinct from the skill's own name, so the weighted path (not the
        // name/phrase bypass) is what's being exercised.
        let skills = vec![
            skill("skill-alpha", "kyverno policy enforcement for kubernetes"),
            skill("skill-beta", "generic automation helper for deployments"),
            skill("skill-gamma", "generic automation helper for pipelines"),
            skill("skill-delta", "generic automation helper for releases"),
        ];
        let idf = compute_idf(&skills);
        let keywords = build_skill_keywords(&skills[0], &idf);

        unsafe {
            std::env::set_var("SKILLSCOPE_TFIDF_THRESHOLD", "1.0");
        }
        let matching_text = "how does kyverno policy enforcement work here";
        let tokens: HashSet<String> = tokenize(matching_text).into_iter().collect();
        assert!(prompt_matches_skill(matching_text, &tokens, &keywords));

        unsafe {
            std::env::set_var("SKILLSCOPE_TFIDF_THRESHOLD", "1000.0");
        }
        let tokens2: HashSet<String> = tokenize(matching_text).into_iter().collect();
        assert!(
            !prompt_matches_skill(matching_text, &tokens2, &keywords),
            "an absurdly high threshold must suppress the weighted-path match"
        );
        unsafe {
            std::env::remove_var("SKILLSCOPE_TFIDF_THRESHOLD");
        }
    }

    #[test]
    fn tfidf_threshold_env_override_and_default() {
        let _guard = ENV_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("SKILLSCOPE_TFIDF_THRESHOLD");
        }
        assert_eq!(tfidf_threshold(), 20.0);
        unsafe {
            std::env::set_var("SKILLSCOPE_TFIDF_THRESHOLD", "5.5");
        }
        assert_eq!(tfidf_threshold(), 5.5);
        unsafe {
            std::env::remove_var("SKILLSCOPE_TFIDF_THRESHOLD");
        }
    }

    #[test]
    fn min_session_count_env_override_and_default() {
        let _guard = ENV_GUARD.lock().unwrap();
        unsafe {
            std::env::remove_var("SKILLSCOPE_MIN_SESSION_COUNT");
        }
        assert_eq!(min_session_count(), 8);
        unsafe {
            std::env::set_var("SKILLSCOPE_MIN_SESSION_COUNT", "3");
        }
        assert_eq!(min_session_count(), 3);
        unsafe {
            std::env::remove_var("SKILLSCOPE_MIN_SESSION_COUNT");
        }
    }

    // -- is_noise_prompt -----------------------------------------------------------

    #[test]
    fn noise_prompt_filters_plan_injection_teammate_message_and_short_text() {
        assert!(is_noise_prompt(
            "Implement the following plan: do the thing"
        ));
        assert!(is_noise_prompt(
            "<teammate-message>hello there</teammate-message>"
        ));
        assert!(is_noise_prompt("short"));
        assert!(!is_noise_prompt(
            "this is a perfectly normal real user prompt"
        ));
    }
}
