"""skillscope CLI: argparse subcommands over parsed skill invocations."""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import asdict
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable

from rich.console import Console
from rich.table import Table

from .aggregate import project_counts, sessions_for_skill, skill_counts, timeline
from .models import SkillInvocation
from .parser import iter_invocations

DEFAULT_PROJECTS_DIR = Path.home() / ".claude" / "projects"


def _load_invocations(projects_dir: Path, since: datetime | None) -> list[SkillInvocation]:
    invs = iter_invocations(projects_dir)
    if since is not None:
        invs = (inv for inv in invs if inv.timestamp >= since)
    return list(invs)


def _parse_since(value: str | None) -> datetime | None:
    if value is None:
        return None
    try:
        return datetime.strptime(value, "%Y-%m-%d").replace(tzinfo=timezone.utc)
    except ValueError as exc:
        raise argparse.ArgumentTypeError(f"--since must be YYYY-MM-DD, got {value!r}") from exc


def _inv_to_dict(inv: SkillInvocation) -> dict:
    d = asdict(inv)
    d["trigger_type"] = inv.trigger_type.value
    d["timestamp"] = inv.timestamp.isoformat()
    return d


def _print_json(data) -> None:
    print(json.dumps(data, indent=2, default=str))


def cmd_summary(args: argparse.Namespace, console: Console) -> None:
    invs = _load_invocations(args.projects_dir, args.since)
    counts = skill_counts(invs)
    rows = sorted(counts.items(), key=lambda kv: kv[1]["total"], reverse=True)

    if args.json:
        _print_json(
            {
                name: {
                    **{k: v for k, v in stats.items() if k not in ("first_seen", "last_seen")},
                    "first_seen": stats["first_seen"].isoformat(),
                    "last_seen": stats["last_seen"].isoformat(),
                }
                for name, stats in rows
            }
        )
        return

    table = Table(title="Skill invocation summary")
    table.add_column("Skill")
    table.add_column("Total", justify="right")
    table.add_column("User /slash", justify="right")
    table.add_column("Claude proactive", justify="right")
    table.add_column("First seen")
    table.add_column("Last seen")
    for name, stats in rows:
        table.add_row(
            name,
            str(stats["total"]),
            str(stats["user_slash"]),
            str(stats["claude_proactive"]),
            stats["first_seen"].date().isoformat(),
            stats["last_seen"].date().isoformat(),
        )
    console.print(table)


def cmd_sessions(args: argparse.Namespace, console: Console) -> None:
    invs = _load_invocations(args.projects_dir, args.since)
    rows = sessions_for_skill(invs, args.skill)

    if args.json:
        _print_json(
            [
                {
                    **{k: v for k, v in row.items() if k not in ("first_ts", "last_ts")},
                    "first_ts": row["first_ts"].isoformat(),
                    "last_ts": row["last_ts"].isoformat(),
                }
                for row in rows
            ]
        )
        return

    table = Table(title=f"Sessions invoking '{args.skill}'")
    table.add_column("Session ID")
    table.add_column("Project")
    table.add_column("Count", justify="right")
    table.add_column("First")
    table.add_column("Last")
    for row in rows:
        table.add_row(
            row["session_id"],
            row["project_path"],
            str(row["count"]),
            row["first_ts"].isoformat(timespec="minutes"),
            row["last_ts"].isoformat(timespec="minutes"),
        )
    console.print(table)


def cmd_timeline(args: argparse.Namespace, console: Console) -> None:
    invs = _load_invocations(args.projects_dir, args.since)
    granularity = "week" if args.week else "day"
    series = timeline(invs, skill=args.skill, granularity=granularity)

    if args.json:
        _print_json(series)
        return

    title = f"Timeline ({granularity})"
    if args.skill:
        title += f" for '{args.skill}'"
    table = Table(title=title)
    table.add_column("Period")
    table.add_column("Count", justify="right")
    for period, count in series.items():
        table.add_row(period, str(count))
    console.print(table)


def cmd_projects(args: argparse.Namespace, console: Console) -> None:
    invs = _load_invocations(args.projects_dir, args.since)
    counts = project_counts(invs)
    rows = sorted(counts.items(), key=lambda kv: kv[1]["total"], reverse=True)

    if args.json:
        _print_json({project: stats for project, stats in rows})
        return

    table = Table(title="Per-project skill usage")
    table.add_column("Project")
    table.add_column("Total", justify="right")
    table.add_column("Top skills")
    for project, stats in rows:
        top = ", ".join(f"{name} ({count})" for name, count in stats["top_skills"])
        table.add_row(project, str(stats["total"]), top)
    console.print(table)


def cmd_fidelity(args: argparse.Namespace, console: Console) -> None:
    try:
        from .fidelity import run_fidelity
    except ImportError:
        console.print("[red]fidelity module not available[/red]")
        sys.exit(1)

    report = run_fidelity(args.projects_dir)

    if args.json:
        _print_json(
            {
                "under_triggered": [
                    {"skill_name": i.skill_name, "evidence": i.evidence, "count": i.count}
                    for i in report.under_triggered
                ],
                "over_triggered": [
                    {"skill_name": i.skill_name, "evidence": i.evidence, "count": i.count}
                    for i in report.over_triggered
                ],
            }
        )
        return

    under = Table(title="Under-triggered skills (matched intent, never fired)")
    under.add_column("Skill")
    under.add_column("Count", justify="right")
    under.add_column("Evidence")
    for item in sorted(report.under_triggered, key=lambda i: i.count, reverse=True):
        under.add_row(item.skill_name, str(item.count), item.evidence)
    console.print(under)

    over = Table(title="Over-triggered skills (fired on unrelated prompts)")
    over.add_column("Skill")
    over.add_column("Count", justify="right")
    over.add_column("Evidence")
    for item in sorted(report.over_triggered, key=lambda i: i.count, reverse=True):
        over.add_row(item.skill_name, str(item.count), item.evidence)
    console.print(over)


def cmd_export(args: argparse.Namespace, console: Console) -> None:
    invs = _load_invocations(args.projects_dir, args.since)
    try:
        for inv in invs:
            print(json.dumps(_inv_to_dict(inv), default=str))
    except BrokenPipeError:
        sys.stderr.close()


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="skillscope",
        description="Claude Code skill-invocation analytics (local JSONL transcripts only).",
    )
    parser.add_argument(
        "--projects-dir",
        type=Path,
        default=DEFAULT_PROJECTS_DIR,
        help=f"Directory containing Claude Code project transcripts (default: {DEFAULT_PROJECTS_DIR})",
    )
    parser.add_argument(
        "--since",
        type=_parse_since,
        default=None,
        help="Only include invocations on/after this date (YYYY-MM-DD)",
    )
    parser.add_argument("--json", action="store_true", help="Emit machine-readable JSON output")

    subparsers = parser.add_subparsers(dest="command", required=True)

    p_summary = subparsers.add_parser("summary", help="Per-skill counts and trigger breakdown")
    p_summary.set_defaults(func=cmd_summary)

    p_sessions = subparsers.add_parser("sessions", help="Session drill-down for one skill")
    p_sessions.add_argument("skill", help="Skill name")
    p_sessions.set_defaults(func=cmd_sessions)

    p_timeline = subparsers.add_parser("timeline", help="Time-series of invocations")
    p_timeline.add_argument("skill", nargs="?", default=None, help="Optional skill name to scope to")
    p_timeline.add_argument("--week", action="store_true", help="Group by week instead of day")
    p_timeline.set_defaults(func=cmd_timeline)

    p_projects = subparsers.add_parser("projects", help="Per-project skill usage breakdown")
    p_projects.set_defaults(func=cmd_projects)

    p_fidelity = subparsers.add_parser("fidelity", help="Trigger-fidelity report")
    p_fidelity.set_defaults(func=cmd_fidelity)

    p_export = subparsers.add_parser("export", help="JSON-lines export of normalized invocations")
    p_export.set_defaults(func=cmd_export)

    return parser


def main(argv: list[str] | None = None) -> None:
    parser = build_parser()
    args = parser.parse_args(argv)
    console = Console()
    args.func(args, console)


if __name__ == "__main__":
    main()
