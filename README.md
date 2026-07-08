# skillscope

Claude Code skill-invocation analytics. Local-only: parses `~/.claude/projects/**/*.jsonl` transcripts directly — no Enterprise admin console, no org API, no OTLP sink required. Works for any individual Claude Code install.

Rust is the primary, shipped implementation (this directory), promoted after a bake-off against Go and Python POCs (see `experiments/`).

## Agent support

Claude Code only, currently. skillscope parses `~/.claude/projects/**/*.jsonl` — Claude Code's
specific transcript format (`sessionId`/`cwd`/`timestamp` per line, `<command-name>` tags for
slash invocations, `Skill` tool_use blocks for proactive ones, `sessions-index.json` per
project).

Codex (`~/.codex/sessions/`) and OpenCode (`~/.local/share/opencode/`) both log equivalent
per-session JSONL/JSON transcripts with a comparable shape (turn-by-turn messages, tool calls,
timestamps, cwd). Adding either as a second `Origin`/data source is expected to be a parser +
model addition, not a rearchitecture: implement a `parser::<agent>` module producing the same
`SkillInvocation` (or agent-equivalent "tool/command invocation") struct the aggregation,
fidelity, and TUI layers already consume, and gate it behind a `--agent claude|codex|opencode`
flag or auto-detection by which directory exists. No PRs open for this yet.

## What it does

- **Per-skill usage counts** with trigger-type breakdown (user `/slash` vs model-proactive `Skill` tool_use)
- **Session-level drill-down** — which sessions fired a skill, in which project/cwd
- **Session-scoped modes** — `skillscope .` (fzf picker over sessions for the current cwd) and
  `skillscope <session-id>` (full UUID or `>=8`-char hex prefix) open a TUI scoped to one session,
  toggled between a skills-in-session table and a flat chronological timeline (`Tab`)
- **Temporal firing patterns** — daily/weekly cadence, trends, dead periods
- **Trigger-fidelity evals** — under-triggering (skill matches intent but never fired) and
  over-triggering (fired on unrelated prompts), TF-IDF weighted against each skill's frontmatter
  `description`
- **Origin tracking** — distinguishes invocations from main-session transcripts vs subagent
  transcripts (`--origin main|subagent`)
- **Skill inventory** — join installed skills (`~/.agents/skills`, `~/.claude/skills`, plugin
  marketplaces) against invocation history to see what's installed but never fires

## Prerequisites

A Rust toolchain (edition 2021, stable). If you already manage Rust with
[rustup](https://rustup.rs/) or your distro's packages, use that — skillscope has no special
toolchain requirements.

If you have no existing preference, [asdf](https://asdf-vm.com/) with the
[asdf-rust plugin](https://github.com/asdf-community/asdf-rust) keeps the toolchain pinned
per-project:

```bash
asdf plugin add rust https://github.com/asdf-community/asdf-rust.git
asdf install rust latest
asdf set rust latest          # writes .tool-versions in the repo
```

## Install / run

```bash
make build && make install    # builds target/release/skillscope, installs to ~/bin

skillscope                    # TUI: skills -> sessions -> invocations drill-down
skillscope .                  # fzf picker over sessions for the current cwd -> scoped TUI
skillscope <session-id>       # scoped TUI for one session (full UUID or >=8-char hex prefix)

skillscope summary            # per-skill counts + trigger breakdown
skillscope sessions <skill>   # session drill-down for one skill
skillscope timeline [skill]   # time-series (daily/weekly)
skillscope projects           # per-project breakdown
skillscope fidelity           # trigger-fidelity report
skillscope report [skill]     # per-cwd session survey with trigger context
skillscope inventory [skill]  # installed-skill inventory joined against invocation history
skillscope export             # JSON export of normalized invocations
```

## Data sources (JSONL schema, confirmed against live transcripts)

1. **User slash invocation** — `type:"user"` line, `message.content` is a string containing
   `<command-name>/foo</command-name>` (plus `<command-message>`, `<command-args>`).
2. **Model-proactive invocation** — `type:"assistant"` line, `message.content[]` entry with
   `type:"tool_use"`, `name:"Skill"`, `input.skill` = skill name, optional `input.args`.
3. **Subagent transcripts** — `<project>/<session-uuid>/subagents/agent-*.jsonl`, same schema,
   tagged with `origin: subagent`.
4. **`sessions-index.json`** — Claude Code's own per-project session index
   (`firstPrompt`/`summary`/`gitBranch`/`modified`/`projectPath`), joined for friendlier session
   labels and recency sorting.

Each transcript line also carries `sessionId`, `cwd`, `timestamp` (ISO 8601).

## Data retention (read this before trusting long-range trends)

Claude Code deletes local session transcripts under `~/.claude/projects/` after **30 days** by
default — this is the `cleanupPeriodDays` setting, and the cleanup sweep runs at every startup.
See [Data usage: Data retention](https://code.claude.com/docs/en/data-usage#data-retention) and
[Settings: cleanupPeriodDays](https://code.claude.com/docs/en/settings#available-settings) in the
official docs.

This caps how far back `skillscope timeline`, `report`, and `fidelity` can meaningfully look —
anything older than the retention window is already gone from disk by the time you run the tool,
regardless of skillscope's own logic. A month is thin for spotting slow-moving trends (a skill
that fires quarterly, a fidelity regression that crept in over months). If you want a longer
retrospective window, raise the limit **before** you need the history, since it only protects
transcripts going forward:

```json
// ~/.claude/settings.json
{
  "cleanupPeriodDays": 180
}
```

`0` is invalid (fails validation) and is not "keep forever" — use a large value instead (e.g.
`9999`) if you want retention to be effectively indefinite. Raising this trades disk space
(transcripts are plaintext JSONL, and can be sizeable on an active install) for analytics depth;
there's no partial option — it's a single global setting, not "keep only skillscope-relevant
sessions."

## Architecture

```
src/
├── models.rs        # SkillInvocation, Origin, TriggerType
├── parser.rs        # JSONL streaming extraction (main + subagent + scoped)
├── aggregate.rs      # counts, trigger breakdown, time-series, per-project rollups
├── sessions.rs        # sessions-index.json join
├── sessionscan.rs      # cwd -> session discovery for the `.` picker
├── resolve.rs           # session-id / hex-prefix resolution
├── fzf.rs                # fzf picker wrapper (line build/parse, process I/O)
├── fidelity.rs            # skill discovery + TF-IDF trigger-fidelity heuristics
├── report.rs               # per-cwd session survey
├── inventory.rs             # installed-skill inventory join
├── cli.rs                    # clap subcommands + global flags
└── tui/                       # ratatui: global + session-scoped drill-down views
```

## Fidelity tunables

`SKILLSCOPE_TFIDF_THRESHOLD` (default 20.0) and `SKILLSCOPE_MIN_SESSION_COUNT` (default 8) are
env-overridable to tune the trigger-fidelity heuristic without a code change.

## Agent skill

An [Agent Skill](https://agentskills.io/) ships in-repo at
[`.agents/skills/skillscope/`](.agents/skills/skillscope/) so a coding agent can answer
skill-usage questions on request ("what skills did session `abc12345` invoke, when, and how?")
by driving `skillscope export --json | jq` non-interactively. It's picked up automatically by
agents that read `.agents/skills/` at project scope; for global availability, symlink or copy it
into `~/.agents/skills/`.

## Other implementations

Two earlier bake-off entries are kept as reference/experiments, not shipped:

- [`experiments/python/`](experiments/python/) — the original Python POC; still the parity oracle
  `make parity` checks the Rust `export` output against
- [`experiments/go/`](experiments/go/) — a Go + Bubbletea rewrite from the same bake-off

## License

[MIT](LICENSE)
