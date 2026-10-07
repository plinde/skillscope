---
name: skillscope
version: 1.2.0
description: "Analyze skill-invocation history from local Claude Code (plus Codex, pi, opencode) transcripts with the `skillscope` CLI: what fired, when, how. Use for \"what skills did session X invoke\", \"is my-skill firing\", project or time-window surveys, never-fired skills. Local-only, read-only."
---

# skillscope

## When to use

Analyze Claude Code skill-invocation history from local transcripts on request — what skills fired, when, in what context, and how (user `/slash`, user-named, model-proactive, direct SKILL.md read, subagent). Use when the user asks to inspect a specific session ("what skills did session abc123 invoke and when"), audit a skill's usage ("is my-skill actually firing / when did it last run"), survey activity in a project/cwd, spot trends over time, or find installed-but-never-fired skills. Backed by the `skillscope` CLI over `~/.claude/projects/**/*.jsonl` (plus Codex, pi and opencode history via `--harness`); drive it non-interactively with `export --json | jq`. Local-only, no admin console or org API.

Answer questions about how Claude Code skills are being invoked, from the local JSONL transcripts
under `~/.claude/projects/`. The job is **analysis on request** — the user names a session, a skill,
a project, or a time window, and you return a precise, evidence-backed answer.

## What the data contains

skillscope normalizes every skill invocation into a flat record. One JSON object per invocation
(from `skillscope export --json`):

| Field | Meaning |
|-------|---------|
| `skill_name` | The skill invoked |
| `trigger_type` | **How** it fired: `user-slash` (user typed `/skill`), `user-named` (model called the Skill tool after the user's latest prose named it as `/skill` or `$skill`), `claude-proactive` (model called the Skill tool unprompted), or `direct-read` (SKILL.md loaded with Read/Bash/`exec_command` instead of the Skill tool) |
| `harness` | `claude`, `codex`, `pi` or `opencode` — which agent recorded it |
| `origin` | `main` (main session transcript) or `subagent` (fired inside a spawned subagent) |
| `session_id` | Full session UUID |
| `timestamp` | ISO-8601 UTC — **when** it fired |
| `args` | Arguments/prompt passed to the skill (the **context** — often the richest signal); may be `null` |
| `project_path` | The `cwd` the session ran in |
| `transcript_file` | Absolute path to the source `.jsonl` (for drilling into surrounding turns) |

The three axes the user usually asks about map to: **when** = `timestamp`, **what** = `skill_name`
+ `args`, **how** = `trigger_type` (+ `origin` for main-vs-subagent).

## Prerequisite: the binary

Requires the `skillscope` CLI on `PATH`. Check, and build from the repo root if missing:

```bash
command -v skillscope || (cd "$(git rev-parse --show-toplevel)" && make build && make install)
```

## Primary workflow: `export --json | jq`

The subcommands' default output is human tables, and the bare `skillscope` / `skillscope <TARGET>`
forms open an interactive TUI — **not** usable non-interactively. For agent-driven analysis, always
go through `export --json`, which streams every invocation as JSON lines, then slice with `jq`.

Global filters that apply to every command: `--since <YYYY-MM-DD | 7d | 30d>`, `--origin main|subagent`,
`--projects-dir <dir>` (point at a non-default transcript root), `--harness claude|codex|pi|opencode|all`
(default `claude`; Codex/pi records are all `direct-read`, opencode `skill`-tool calls are
`claude-proactive`; `report`, `fidelity` and the session TUIs stay Claude-only).

### "What skills did session `abc12345` invoke, when, and how?"

Session ids are long UUIDs; users usually give a prefix. Match on `startswith` (the CLI's own
resolver requires ≥8 hex chars to disambiguate — apply the same discipline):

```bash
PFX=abc12345   # >=8 chars
skillscope export --json \
  | jq -c --arg p "$PFX" 'select(.session_id|startswith($p))
      | {when:.timestamp, skill:.skill_name, how:.trigger_type, origin, context:.args}' \
  | sort
```

Present it as a chronological timeline: timestamp → skill → trigger type, with `args` as the
context for why it fired. Call out the trigger split explicitly — that's the "how" the user asked
for. If the prefix matches more than one `session_id`, stop and report the
candidates (don't silently pick one).

### "How often did each trigger type fire in that session?"

```bash
skillscope export --json \
  | jq -r --arg p "$PFX" 'select(.session_id|startswith($p)) | .trigger_type' \
  | sort | uniq -c
```

### "Is `<skill>` actually being called / when did it last run?"

```bash
# every invocation of one skill, newest last
skillscope export --json \
  | jq -c --arg s "<skill>" 'select(.skill_name==$s)
      | {when:.timestamp, how:.trigger_type, origin, session:.session_id, context:.args}' \
  | sort
```

Or use the purpose-built commands for a shaped answer:
- `skillscope inventory <skill> --json` — installed-skill detail incl. last-run ("when did abc last run?").
- `skillscope inventory --json` — full installed-vs-fired join; find skills installed but never invoked.
- `skillscope sessions <skill> --json` — which sessions fired one skill.

### "What's the skill usage in this project / sessions started here?"

```bash
skillscope report --json                 # sessions started from the current cwd
skillscope report <skill> --cwd <dir> --json   # focus one skill, one directory
skillscope projects --json               # per-project rollup across everything
```

### "Overall counts / trends"

```bash
skillscope summary --json                # per-skill totals + user_slash/user_named/claude_proactive/direct_read/subagent split
skillscope summary --harness all --json  # same, across Claude Code, Codex, pi and opencode
skillscope timeline --json               # daily series; add --week for weekly; add a [skill] to scope
skillscope timeline <skill> --since 30d --json
```

### "Trigger fidelity — is a skill under- or over-firing?"

```bash
skillscope fidelity --json
```

Flags skills that match a prompt's intent but never fired (under-triggering) and skills that fired
on unrelated prompts (over-triggering), TF-IDF-weighted against each skill's frontmatter
`description`. Tunables (env): `SKILLSCOPE_TFIDF_THRESHOLD` (default 20.0),
`SKILLSCOPE_MIN_SESSION_COUNT` (default 8).

### "Is this skill's reference read in full, or only in part?"

```bash
skillscope refs <skill> --harness all --since 30d          # per bundled file
skillscope refs <skill> --harness all --by-model --json    # split by model
skillscope refs <skill> --reads | jq -c '{rel_path, extent, tool, model, session_id}'
```

Per file: depth below SKILL.md, lines, TOC, reads split into `full` / `partial` (`head -N`, Read
`limit`, `sed -n 'a,bp'`) / `search` (`grep`/`rg`) / other (script runs). `!` marks a file of a
skill used in the window that was never read. Check a surprising row with `--reads`.

### "Lint my skills"

```bash
skillscope lint                      # every installed skill: manifest, vendored, plugin
skillscope lint <skill> --fail       # exit 1 on any finding (for scripts and PR checks)
```

Rules: `nested-ref` (a reference links another reference), `ref-over-100` (split by domain;
`ref-over-100-toc` if it has a contents list), `orphan-file` (nothing reachable from SKILL.md names
it), `body-over-500`. Each finding names its docs anchor or "local policy".

## Interactive mode (hand back to the user)

`skillscope` (global TUI) and `skillscope .` / `skillscope <session-id>` (session-scoped TUI, `Tab`
toggles skills-table vs chronological timeline) are for a human at a terminal. You can't drive them
— when a user wants to browse interactively rather than get an analyzed answer, tell them to run the
command themselves.

## Answering well

- **Lead with the answer**, then the evidence. The user asked a question; the table is support.
- **Always distinguish trigger types** — `user-slash` / `user-named` (user-directed) vs
  `claude-proactive` (genuine description trigger) vs `direct-read` (file loaded around the Skill
  tool), plus `subagent` origin, is usually the point of the question, not an afterthought. Only
  `claude-proactive` is evidence the description itself triggers the skill.
- **Use `args` as context.** When explaining *why* a skill fired, the args/prompt is the strongest
  signal; quote or summarize it.
- **Timestamps are UTC.** Say so if local time matters to the user.
- **Mind retention.** Claude Code deletes transcripts older than `cleanupPeriodDays` (default 30) at
  startup, so history older than that window is already gone from disk — flag this when a user asks
  about long-range trends and the data looks truncated.
- **Don't fabricate.** If a session prefix matches nothing, or a skill has zero invocations, say so
  plainly rather than guessing.
