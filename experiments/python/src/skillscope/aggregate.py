"""Pure aggregation functions over SkillInvocation streams.

No I/O here — everything takes an ``Iterable[SkillInvocation]`` (typically
from ``parser.iter_invocations``) and returns plain dict/list structures.
Callers materialize/filter the iterable (e.g. by --since) before calling in.
"""

from __future__ import annotations

from collections import OrderedDict, defaultdict
from datetime import date, datetime, timedelta
from typing import Iterable, Optional

from .models import SkillInvocation, TriggerType


def skill_counts(invs: Iterable[SkillInvocation]) -> dict[str, dict]:
    """Per-skill totals, trigger-type breakdown, and first/last-seen timestamps."""
    counts: dict[str, dict] = {}
    for inv in invs:
        entry = counts.setdefault(
            inv.skill_name,
            {
                "total": 0,
                "user_slash": 0,
                "claude_proactive": 0,
                "first_seen": inv.timestamp,
                "last_seen": inv.timestamp,
            },
        )
        entry["total"] += 1
        if inv.trigger_type == TriggerType.USER_SLASH:
            entry["user_slash"] += 1
        elif inv.trigger_type == TriggerType.CLAUDE_PROACTIVE:
            entry["claude_proactive"] += 1
        if inv.timestamp < entry["first_seen"]:
            entry["first_seen"] = inv.timestamp
        if inv.timestamp > entry["last_seen"]:
            entry["last_seen"] = inv.timestamp
    return counts


def sessions_for_skill(invs: Iterable[SkillInvocation], skill: str) -> list[dict]:
    """Sessions that fired ``skill``, with per-session count and time span."""
    sessions: dict[str, dict] = {}
    for inv in invs:
        if inv.skill_name != skill:
            continue
        entry = sessions.setdefault(
            inv.session_id,
            {
                "session_id": inv.session_id,
                "project_path": inv.project_path,
                "count": 0,
                "first_ts": inv.timestamp,
                "last_ts": inv.timestamp,
            },
        )
        entry["count"] += 1
        if inv.timestamp < entry["first_ts"]:
            entry["first_ts"] = inv.timestamp
        if inv.timestamp > entry["last_ts"]:
            entry["last_ts"] = inv.timestamp
    return sorted(sessions.values(), key=lambda e: e["count"], reverse=True)


def _period_key(ts: datetime, granularity: str) -> str:
    if granularity == "week":
        monday = ts.date() - timedelta(days=ts.date().weekday())
        return monday.isoformat()
    return ts.date().isoformat()


def timeline(
    invs: Iterable[SkillInvocation],
    skill: Optional[str] = None,
    granularity: str = "day",
) -> "OrderedDict[str, int]":
    """Ordered (ascending) period -> invocation count, optionally scoped to one skill."""
    if granularity not in ("day", "week"):
        raise ValueError(f"granularity must be 'day' or 'week', got {granularity!r}")

    counts: dict[str, int] = defaultdict(int)
    for inv in invs:
        if skill is not None and inv.skill_name != skill:
            continue
        counts[_period_key(inv.timestamp, granularity)] += 1

    return OrderedDict(sorted(counts.items()))


def project_counts(invs: Iterable[SkillInvocation]) -> dict[str, dict]:
    """Per-project totals plus each project's top-5 skills by count."""
    totals: dict[str, int] = defaultdict(int)
    per_skill: dict[str, dict[str, int]] = defaultdict(lambda: defaultdict(int))

    for inv in invs:
        totals[inv.project_path] += 1
        per_skill[inv.project_path][inv.skill_name] += 1

    result: dict[str, dict] = {}
    for project, total in totals.items():
        top_skills = sorted(per_skill[project].items(), key=lambda kv: kv[1], reverse=True)[:5]
        result[project] = {"total": total, "top_skills": top_skills}
    return result
