# skillscope (Python POC)

Original Python proof-of-concept for skillscope. Superseded by the Rust implementation at the
repo root — kept here as the parity oracle `../../scripts/parity.sh` (`make parity` from the repo
root) checks the Rust `export` output against, and as a reference for the core parsing/fidelity
logic.

Not the shipped tool. See the [root README](../../README.md) for current status.

## Run

```bash
uv run skillscope summary            # per-skill counts + trigger breakdown
uv run skillscope sessions <skill>   # session drill-down for one skill
uv run skillscope timeline [skill]   # time-series (daily/weekly)
uv run skillscope projects           # per-project breakdown
uv run skillscope fidelity           # trigger-fidelity report
uv run skillscope export             # JSON export of normalized invocations
```

## Known limitations (frozen as of the bake-off)

- Fidelity heuristic is deliberately loose (`>=2` keyword hits = plausible match); the Rust
  implementation replaced this with TF-IDF weighting.
- Subagent transcripts (`<session>/subagents/agent-*.jsonl`) are excluded — skill invocations
  made *by subagents* are not counted.
- `sessions-index.json` is not joined for friendlier session labels.

## Architecture

```
src/skillscope/
├── models.py     # SkillInvocation, SkillDefinition, UserPrompt, TriggerType
├── parser.py     # JSONL streaming extraction → SkillInvocation / UserPrompt
├── aggregate.py  # counts, trigger breakdown, time-series, per-project rollups
├── fidelity.py   # skill discovery + trigger-fidelity heuristics
└── cli.py        # argparse subcommands, rich tables, JSON export
```
