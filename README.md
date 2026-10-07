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
- **Skill lint** — check installed skills (manifest, vendored, plugin) against the reference-file
  rules from Claude's skill best practices: nested references, references over 100 lines,
  bundled files nothing links to, SKILL.md bodies over 500 lines (`--fail` for scripts)
- **Reference reads** — per bundled file (`references/*.md`, `scripts/*`): how deep it sits below
  SKILL.md, its size and TOC, and whether transcripts read it in full, in part (`head -100`,
  Read `limit`) or only searched it, by which model; files of a used skill that are never read

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
brew install plinde/tap-private/skillscope   # release build (macOS arm64, Linux x86_64)
make build                    # dev build: target/release/skillscope

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
skillscope lint [skill]       # reference-file rules for installed skills (--fail: exit 1 on findings)
skillscope refs [skill]       # per bundled file: depth, lines, TOC, full/partial/search reads (--by-model)
skillscope refs <skill> --reads  # the matched tool calls as JSON lines
skillscope export             # JSON export of normalized invocations

skillscope summary --harness all   # include Codex, pi and opencode history (default: claude)
```

## Releases

Pushing a `v*` tag that matches `Cargo.toml`'s version runs `.github/workflows/release.yml`:
it builds `aarch64-apple-darwin` (ad-hoc codesigned) and `x86_64-unknown-linux-musl` (static,
any glibc), publishes a GitHub Release, and pushes `Formula/skillscope.rb` to
`plinde/homebrew-tap-private` (needs the `TAP_GITHUB_TOKEN` secret); `brew upgrade` picks it up.
PRs that touch the build inputs, and `workflow_dispatch`, run the builds only.

```bash
# bump version in Cargo.toml via PR, merge, then:
git tag v0.2.0 origin/main && git push origin v0.2.0
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
5. **Reference reads** (`skillscope refs`, a separate pass: invocation counts don't change) — any
   tool call naming `skills/<name>/<relpath>` with relpath not `SKILL.md`, in Claude, Codex and pi
   transcripts. The extent comes from the tool's own fields:
   - **full** — Read with no `limit`/`offset`; `cat`, `nl`; `sed -n '1,$p'`.
   - **partial** — Read `limit` (or `offset`); `head`/`tail` (`-N`, `-n N`; default 10);
     `sed -n 'a,bp'`; `awk 'NR<=N'`. A `cat`/`nl` piped into one of these takes its bound.
   - **search** — `grep`/`rg`, the Grep tool.
   - **unknown** — anything else naming the file (running a script, `less`/`more`).

   Shell commands are split at `|`, `&&`, `;` and only the segment naming the file decides.
   Commands that don't read text (`ls`, `wc`, `git`, `sed -i`) and tools that only quote paths
   (plans, todos) are skipped. The model is the assistant `message.model` (Claude, pi) or the
   Codex `turn_context.model`. One record per file per message.
6. **`sessions-index.json`** — Claude Code's own per-project session index
   (`firstPrompt`/`summary`/`gitBranch`/`modified`/`projectPath`), joined for friendlier session
   labels and recency sorting.

Each transcript line also carries `sessionId`, `cwd`, `timestamp` (ISO 8601). Every exported
record carries `harness` (`claude`, `codex`, `pi`, `opencode`).

## Lint rules

From Claude's [skill authoring best practices](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices):
Claude may preview nested reference files with `head -100` and so read them only in part.

| Rule | Level | Fires when | Source |
|---|---|---|---|
| `nested-ref` | error | a reference `.md` links another reference `.md` (markdown link or backticked name, outside code fences) | `#avoid-deeply-nested-references` |
| `ref-over-100` | error | a reference `.md` is over 100 lines with no table of contents: split it by domain | local policy, `#pattern-2-domain-specific-organization` |
| `ref-over-100-toc` | warn | as above, but a `## Contents` heading sits in its first 30 lines | `#structure-longer-reference-files-with-table-of-contents` |
| `orphan-file` | warn | a bundled file nothing reachable from SKILL.md names (path, link, or a parent directory) | `#observe-how-claude-navigates-skills` |
| `body-over-500` | error | SKILL.md body (after frontmatter) is over 500 lines | `#token-budgets` |

`evals/`, `agents/openai.yaml` (Codex metadata), `LICENSE*` and dot-dirs are not bundled content.

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
├── models.rs        # SkillInvocation, RefRead, ReadExtent, Origin, TriggerType, Harness
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
├── skilltree.rs             # skill dir walk: bundled files, links, depth, lint rules
├── refreads.rs              # bundled-file reads + extent (full/partial/search) per tool call
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
