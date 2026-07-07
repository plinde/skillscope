"""Trigger-fidelity eval layer: skill discovery + offline heuristic classification.

No LLM calls. Correlates real user prompts against skill frontmatter descriptions
to flag two failure modes:

- under-triggering: a skill's description plausibly matches a prompt, but the
  skill was never invoked in that session
- over-triggering: a skill was invoked in a session, but no prompt in that
  session plausibly matches its description
"""

from __future__ import annotations

import re
from collections import defaultdict
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any

import yaml

from skillscope.models import SkillDefinition, UserPrompt

DEFAULT_SKILLS_DIRS: list[Path] = [
    Path.home() / ".agents" / "skills",
    Path.home() / ".claude" / "skills",
]
PLUGIN_MARKETPLACES_DIR = Path.home() / ".claude" / "plugins" / "marketplaces"

_WORD_RE = re.compile(r"[a-z0-9][a-z0-9_-]*")
_TRIGGER_SECTION_RE = re.compile(r"triggers?\s+on\s*:", re.IGNORECASE)
_QUOTED_PHRASE_RE = re.compile(r'"([^"]{2,60})"')

_STOPWORDS: frozenset[str] = frozenset(
    {
        # common English function words
        "a", "an", "the", "and", "or", "but", "if", "then", "else", "when",
        "while", "for", "to", "of", "in", "on", "at", "by", "with", "from",
        "into", "onto", "over", "under", "about", "as", "is", "are", "was",
        "were", "be", "been", "being", "this", "that", "these", "those",
        "it", "its", "you", "your", "yours", "i", "we", "our", "ours",
        "they", "their", "he", "she", "his", "her", "them", "not", "no",
        "do", "does", "did", "can", "could", "should", "would", "will",
        "shall", "may", "might", "must", "also", "than", "such", "any",
        "all", "some", "each", "every", "other", "another", "more", "most",
        "much", "many", "only", "just", "very", "so", "too", "own", "same",
        "here", "there", "what", "which", "who", "whom", "how", "why",
        # generic / domain-neutral words called out in the spec
        "use", "uses", "using", "used", "skill", "skills", "tool", "tools",
        "user", "users", "claude", "agent", "agents", "code", "file",
        "files", "task", "tasks", "will", "want", "wants", "need", "needs",
    }
)


@dataclass
class FidelityFinding:
    skill_name: str
    count: int
    evidence: str


@dataclass
class FidelityReport:
    under_triggered: list[FidelityFinding]
    over_triggered: list[FidelityFinding]


@dataclass(frozen=True)
class _SkillKeywords:
    name: str  # lowercased
    words: frozenset[str]  # single tokens, len >= 4, stopwords removed
    phrases: frozenset[str]  # lowercased multi-word phrases from "Triggers on:" sections


def _parse_frontmatter(content: str) -> dict[str, Any] | None:
    if not content.startswith("---"):
        return None
    parts = content.split("---", 2)
    if len(parts) < 3:
        return None
    try:
        data = yaml.safe_load(parts[1])
    except yaml.YAMLError:
        return None
    if not isinstance(data, dict):
        return None
    return data


def _iter_skill_md_candidates(
    skills_dirs: list[Path],
) -> list[tuple[Path, str]]:
    """Yield (SKILL.md path, source) pairs, deduped by resolved path."""
    seen: set[Path] = set()
    candidates: list[tuple[Path, str]] = []

    for skills_dir in skills_dirs:
        if not skills_dir.is_dir():
            continue
        for entry in sorted(skills_dir.iterdir()):
            skill_md = entry / "SKILL.md"
            if not skill_md.is_file():
                continue
            resolved = skill_md.resolve()
            if resolved in seen:
                continue
            seen.add(resolved)
            candidates.append((skill_md, "user"))

    if PLUGIN_MARKETPLACES_DIR.is_dir():
        for skill_md in sorted(PLUGIN_MARKETPLACES_DIR.glob("*/*/skills/*/SKILL.md")):
            resolved = skill_md.resolve()
            if resolved in seen:
                continue
            seen.add(resolved)
            candidates.append((skill_md, "plugin"))

    return candidates


def discover_skills(skills_dirs: list[Path] | None = None) -> list[SkillDefinition]:
    """Scan skill directories for SKILL.md files and parse their frontmatter.

    Files without valid YAML frontmatter are skipped gracefully.
    """
    dirs = skills_dirs if skills_dirs is not None else DEFAULT_SKILLS_DIRS
    results: list[SkillDefinition] = []

    for skill_md, source in _iter_skill_md_candidates(dirs):
        try:
            content = skill_md.read_text(encoding="utf-8")
        except OSError:
            continue
        frontmatter = _parse_frontmatter(content)
        if frontmatter is None:
            continue
        name = str(frontmatter.get("name") or skill_md.parent.name)
        description = str(frontmatter.get("description") or "")
        results.append(
            SkillDefinition(
                name=name,
                description=description,
                path=str(skill_md.parent),
                source=source,
            )
        )

    return results


def _extract_trigger_phrases(description: str) -> frozenset[str]:
    match = _TRIGGER_SECTION_RE.search(description)
    if not match:
        return frozenset()
    tail = description[match.end():]
    phrases = {p.strip().lower() for p in _QUOTED_PHRASE_RE.findall(tail) if p.strip()}
    return frozenset(phrases)


def _build_skill_keywords(skill: SkillDefinition) -> _SkillKeywords:
    phrases = _extract_trigger_phrases(skill.description)
    text = f"{skill.name} {skill.description}".lower()
    tokens = _WORD_RE.findall(text)
    words = frozenset(t for t in tokens if len(t) >= 4 and t not in _STOPWORDS)
    return _SkillKeywords(name=skill.name.lower(), words=words, phrases=phrases)


def _name_matches(text_lower: str, name_lower: str) -> bool:
    if not name_lower:
        return False
    normalized_name = re.sub(r"[-_]+", " ", name_lower).strip()
    if not normalized_name:
        return False
    normalized_text = re.sub(r"[-_]+", " ", text_lower)
    return f" {normalized_name} " in f" {normalized_text} "


def prompt_matches_skill(
    text_lower: str, tokens: frozenset[str], keywords: _SkillKeywords
) -> bool:
    """A prompt "plausibly matches" a skill per the heuristic rules:

    contains the skill name as a word, OR contains >= 2 distinct keywords
    (len >= 4) from the skill's keyword set, OR contains a quoted trigger
    phrase declared in a "Triggers on:" section of the description.
    """
    if _name_matches(text_lower, keywords.name):
        return True
    if any(phrase in text_lower for phrase in keywords.phrases):
        return True
    return len(tokens & keywords.words) >= 2


def run_fidelity(
    projects_dir: Path | None = None,
    skills_dirs: list[Path] | None = None,
    since: datetime | None = None,
) -> FidelityReport:
    from skillscope.parser import iter_invocations, iter_user_prompts

    skills = discover_skills(skills_dirs)
    skill_keywords = {s.name: _build_skill_keywords(s) for s in skills}

    # iter_invocations/iter_user_prompts default projects_dir themselves;
    # only override when the caller passed an explicit path.
    parser_kwargs: dict[str, Path] = {}
    if projects_dir is not None:
        parser_kwargs["projects_dir"] = projects_dir

    session_invocations: dict[str, set[str]] = defaultdict(set)
    for inv in iter_invocations(**parser_kwargs):
        if since is not None and inv.timestamp < since:
            continue
        session_invocations[inv.session_id].add(inv.skill_name)

    session_prompts: dict[str, list[UserPrompt]] = defaultdict(list)
    for prompt in iter_user_prompts(**parser_kwargs):
        if since is not None and prompt.timestamp < since:
            continue
        session_prompts[prompt.session_id].append(prompt)

    under_counts: dict[str, int] = defaultdict(int)
    under_examples: dict[str, list[str]] = defaultdict(list)
    over_counts: dict[str, int] = defaultdict(int)
    over_examples: dict[str, list[str]] = defaultdict(list)

    all_sessions = set(session_invocations) | set(session_prompts)
    for session_id in all_sessions:
        prompts = session_prompts.get(session_id, [])
        invoked = session_invocations.get(session_id, set())

        session_matched: set[str] = set()
        matched_snippet: dict[str, str] = {}
        for prompt in prompts:
            text_lower = prompt.text.lower()
            tokens = frozenset(_WORD_RE.findall(text_lower))
            for skill_name, keywords in skill_keywords.items():
                if skill_name in session_matched:
                    continue
                if prompt_matches_skill(text_lower, tokens, keywords):
                    session_matched.add(skill_name)
                    matched_snippet[skill_name] = prompt.text[:120]

        for skill_name in session_matched - invoked:
            under_counts[skill_name] += 1
            if len(under_examples[skill_name]) < 3:
                under_examples[skill_name].append(matched_snippet[skill_name])

        for skill_name in invoked - session_matched:
            over_counts[skill_name] += 1
            snippet = prompts[0].text[:120] if prompts else "(no user prompt text in session)"
            if len(over_examples[skill_name]) < 3:
                over_examples[skill_name].append(snippet)

    under_triggered = [
        FidelityFinding(skill_name=name, count=count, evidence=" | ".join(under_examples[name]))
        for name, count in under_counts.items()
        if count >= 3
    ]
    under_triggered.sort(key=lambda f: f.count, reverse=True)

    over_triggered = [
        FidelityFinding(skill_name=name, count=count, evidence=" | ".join(over_examples[name]))
        for name, count in over_counts.items()
    ]
    over_triggered.sort(key=lambda f: f.count, reverse=True)

    return FidelityReport(under_triggered=under_triggered, over_triggered=over_triggered)
