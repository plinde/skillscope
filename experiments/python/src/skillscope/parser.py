"""Streaming JSONL extraction of skill invocations and user prompts.

Reads ``~/.claude/projects/*/*.jsonl`` transcripts line-by-line so the ~700MB
corpus is never fully materialized in memory. Malformed lines are skipped
silently — transcripts are append-only logs written by a live process and
partial/corrupt trailing lines are expected.
"""

from __future__ import annotations

import json
import re
from datetime import datetime
from pathlib import Path
from typing import Iterator

from .models import SkillInvocation, TriggerType, UserPrompt

# CLI built-ins, not skills — excluded from user-slash extraction.
EXCLUDED_COMMANDS = {
    "clear", "model", "help", "config", "compact", "exit", "login", "logout",
    "status", "cost", "doctor", "init", "memory", "export", "resume", "tasks",
    "agents", "mcp", "hooks", "permissions", "terminal-setup", "vim", "bug",
    "release-notes", "upgrade", "usage", "todos",
}

_COMMAND_NAME_RE = re.compile(r"<command-name>\s*/?([^<\s]+)\s*</command-name>")
_COMMAND_ARGS_RE = re.compile(r"<command-args>([^<]*)</command-args>")


def _parse_timestamp(raw: str) -> datetime:
    return datetime.fromisoformat(raw.replace("Z", "+00:00"))


def _decode_project_dir(dir_name: str) -> str:
    """Best-effort decode of a dashes-encoded project directory name into a path.

    The encoding is lossy (real path components can themselves contain
    dashes), so this is a fallback only — used when a transcript line has no
    ``cwd`` of its own.
    """
    decoded = dir_name.replace("-", "/")
    if not decoded.startswith("/"):
        decoded = "/" + decoded
    return decoded


def _load_line(line: str) -> dict | None:
    line = line.strip()
    if not line:
        return None
    try:
        data = json.loads(line)
    except (json.JSONDecodeError, ValueError):
        return None
    return data if isinstance(data, dict) else None


def iter_invocations(
    projects_dir: Path = Path.home() / ".claude" / "projects",
) -> Iterator[SkillInvocation]:
    """Stream SkillInvocation records from every transcript under projects_dir."""
    for jsonl_path in projects_dir.glob("*/*.jsonl"):
        fallback_project_path = _decode_project_dir(jsonl_path.parent.name)
        with jsonl_path.open("r", encoding="utf-8", errors="replace") as f:
            for raw_line in f:
                data = _load_line(raw_line)
                if data is None:
                    continue

                session_id = data.get("sessionId")
                raw_ts = data.get("timestamp")
                if not session_id or not raw_ts:
                    continue
                try:
                    timestamp = _parse_timestamp(raw_ts)
                except ValueError:
                    continue

                message = data.get("message")
                if not isinstance(message, dict):
                    continue

                project_path = data.get("cwd") or fallback_project_path
                line_type = data.get("type")

                if line_type == "user":
                    content = message.get("content")
                    if not isinstance(content, str):
                        continue
                    name_match = _COMMAND_NAME_RE.search(content)
                    if not name_match:
                        continue
                    skill_name = name_match.group(1).strip()
                    if not skill_name or skill_name.lower() in EXCLUDED_COMMANDS:
                        continue
                    args = None
                    args_match = _COMMAND_ARGS_RE.search(content)
                    if args_match:
                        args_text = args_match.group(1).strip()
                        if args_text:
                            args = args_text
                    yield SkillInvocation(
                        skill_name=skill_name,
                        trigger_type=TriggerType.USER_SLASH,
                        session_id=session_id,
                        project_path=project_path,
                        timestamp=timestamp,
                        transcript_file=str(jsonl_path),
                        args=args,
                    )

                elif line_type == "assistant":
                    content = message.get("content")
                    if not isinstance(content, list):
                        continue
                    for entry in content:
                        if not isinstance(entry, dict):
                            continue
                        if entry.get("type") != "tool_use" or entry.get("name") != "Skill":
                            continue
                        tool_input = entry.get("input")
                        if not isinstance(tool_input, dict):
                            continue
                        skill_name = tool_input.get("skill") or tool_input.get("command")
                        if not skill_name:
                            continue
                        args = tool_input.get("args")
                        if not isinstance(args, str) or not args:
                            args = None
                        yield SkillInvocation(
                            skill_name=skill_name,
                            trigger_type=TriggerType.CLAUDE_PROACTIVE,
                            session_id=session_id,
                            project_path=project_path,
                            timestamp=timestamp,
                            transcript_file=str(jsonl_path),
                            args=args,
                        )


def _extract_prompt_text(content: object) -> str | None:
    if isinstance(content, str):
        if not content.startswith("<") and len(content) > 10:
            return content
        return None
    if isinstance(content, list) and content:
        first = content[0]
        if (
            isinstance(first, dict)
            and first.get("type") == "text"
            and isinstance(first.get("text"), str)
            and not first["text"].startswith("<")
            and len(first["text"]) > 10
        ):
            return first["text"]
    return None


def iter_user_prompts(
    projects_dir: Path = Path.home() / ".claude" / "projects",
) -> Iterator[UserPrompt]:
    """Stream real free-text user prompts (for the fidelity layer to correlate)."""
    for jsonl_path in projects_dir.glob("*/*.jsonl"):
        fallback_project_path = _decode_project_dir(jsonl_path.parent.name)
        with jsonl_path.open("r", encoding="utf-8", errors="replace") as f:
            for raw_line in f:
                data = _load_line(raw_line)
                if data is None or data.get("type") != "user":
                    continue
                # Synthetic/meta records are not real user asks: isMeta lines,
                # subagent sidechains (e.g. title-generator prompts), and
                # tool-result carriers all masquerade as type:"user".
                if (
                    data.get("isMeta")
                    or data.get("isSidechain")
                    or data.get("toolUseResult") is not None
                    or data.get("sourceToolAssistantUUID")
                ):
                    continue
                # promptSource "sdk"/"system" marks harness-generated prompts
                # (e.g. conversation-title generators); "typed"/"queued" are
                # real user input, None predates the field — keep those.
                if data.get("promptSource") in ("sdk", "system"):
                    continue

                session_id = data.get("sessionId")
                raw_ts = data.get("timestamp")
                if not session_id or not raw_ts:
                    continue
                try:
                    timestamp = _parse_timestamp(raw_ts)
                except ValueError:
                    continue

                message = data.get("message")
                if not isinstance(message, dict):
                    continue

                text = _extract_prompt_text(message.get("content"))
                if text is None:
                    continue

                project_path = data.get("cwd") or fallback_project_path
                yield UserPrompt(
                    text=text[:500],
                    session_id=session_id,
                    project_path=project_path,
                    timestamp=timestamp,
                )
