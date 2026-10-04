# skillscope

Claude Code skill-invocation analytics. Local-only: parses `~/.claude/projects/**/*.jsonl` transcripts directly (plus Codex, pi and opencode history with `--harness`) — no Enterprise admin console, no org API, no OTLP sink required. Works for any individual Claude Code install.

Rust is the primary, shipped implementation (this directory), promoted after a bake-off against Go and Python POCs (see `experiments/`).

## Agent support

Claude Code by default. `--harness` selects whose history to read:

| `--harness` | Source | What counts as a skill use |
|---|---|---|
| `claude` (default) | `~/.claude/projects/**/*.jsonl` | every trigger type below |
| `codex` | `~/.codex/sessions/**/*.jsonl` rollouts | `direct-read`: a `function_call` / `custom_tool_call` whose arguments reference `skills/<name>/SKILL.md` |
| `pi` | `~/.pi/agent/sessions/*/*.jsonl` | `direct-read`: an assistant `toolCall` whose arguments reference a SKILL.md |
| `opencode` | `~/.local/share/opencode/opencode.db` | `claude-proactive`: a call to opencode's native `skill` tool (read via the `sqlite3` CLI) |
| `all` | every store that exists | — |

Codex and pi have no Skill tool — an agent uses a skill by reading its file — so only tool-call
inputs are matched. Codex's model-visible skill catalog lists every installed SKILL.md path, so
grepping whole lines would count every skill in every session. Session-scoped modes (`skillscope .`,
`skillscope <session-id>`), `report` and `fidelity` stay Claude Code-only.

## What it does

- **Per-skill usage counts** with trigger-type breakdown: user `/slash`, user-named (Skill tool
  call the user asked for by name), model-proactive `Skill` tool_use, and direct SKILL.md reads
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

skillscope summary --harness all   # include Codex, pi and opencode history (default: claude)
```

## Data sources (JSONL schema, confirmed against live transcripts)

1. **User slash invocation** (`user-slash`) — `type:"user"` line, `message.content` is a string
   containing `<command-name>/foo</command-name>` (plus `<command-message>`, `<command-args>`).
2. **Skill tool call** — `type:"assistant"` line, `message.content[]` entry with
   `type:"tool_use"`, `name:"Skill"`, `input.skill` = skill name, optional `input.args`. Split by
   the most recent real user prose before it:
   - **`user-named`** — that prose names the skill as `/foo` or `$foo` (case-insensitive; for a
     plugin skill `plugin:foo`, `/foo` counts too). The user asked for it; the model ran it.
   - **`claude-proactive`** — nothing asked for it; the description/keyword trigger fired.

   "Real user prose" is the text of a `type:"user"` line minus `<system-reminder>` blocks, and
   skips tool results, `isMeta` lines, skill bodies (`Base directory for this skill…`),
   interrupt markers, and anything carrying `<command-name>`, `<task-notification>`,
   `<cross-session-message`, or local-command output. A typed `/command` line counts as the
   user's latest input. Mentions must be whole tokens: `~/x/foo`, `/foo-bar`, `/foo.md` don't
   name `foo`.
3. **Direct read** (`direct-read`) — any other assistant `tool_use` (Read, Bash, Grep, …) whose
   input contains `skills/<name>/SKILL.md`. One record per skill per message (Claude Code writes
   a message's content blocks as separate lines sharing `message.id`). Writes/edits and
   delegations (`Agent`, `Task`, `SendMessage`) are not reads.
4. **Subagent transcripts** — `<project>/<session-uuid>/subagents/agent-*.jsonl`, same schema,
   tagged with `origin: subagent`.
5. **`sessions-index.json`** — Claude Code's own per-project session index
   (`firstPrompt`/`summary`/`gitBranch`/`modified`/`projectPath`), joined for friendlier session
   labels and recency sorting.

Each transcript line also carries `sessionId`, `cwd`, `timestamp` (ISO 8601). Every exported
record carries `harness` (`claude`, `codex`, `pi`, `opencode`).

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
├── models.rs        # SkillInvocation, Origin, TriggerType, Harness
├── parser.rs        # JSONL streaming extraction (main + subagent + scoped)
├── harness.rs       # Codex, pi and opencode session stores
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
