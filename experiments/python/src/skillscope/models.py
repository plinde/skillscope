"""Shared data model for skill invocations extracted from Claude Code JSONL transcripts.

This is the contract between parser, aggregate, fidelity, and cli modules.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from enum import Enum


class TriggerType(str, Enum):
    USER_SLASH = "user-slash"        # <command-name>/foo</command-name> in a type:"user" line
    CLAUDE_PROACTIVE = "claude-proactive"  # tool_use name:"Skill" in a type:"assistant" line


@dataclass(frozen=True)
class SkillInvocation:
    skill_name: str
    trigger_type: TriggerType
    session_id: str
    project_path: str          # decoded cwd from the transcript line (or project dir name)
    timestamp: datetime
    transcript_file: str
    args: str | None = None    # Skill tool_use .input.args, or <command-args> content


@dataclass
class SkillDefinition:
    """A skill discovered on disk, for the fidelity layer."""
    name: str
    description: str           # frontmatter description == the trigger heuristic
    path: str
    source: str = "user"       # user | project | plugin


@dataclass
class UserPrompt:
    """A real user prompt from a transcript, for fidelity classification."""
    text: str
    session_id: str
    project_path: str
    timestamp: datetime
    # skills actually invoked within WINDOW turns after this prompt
    invoked_skills: set[str] = field(default_factory=set)
