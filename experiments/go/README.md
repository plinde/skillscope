# skillscope (Go + Bubbletea POC)

Go + Bubbletea rewrite from the skillscope bake-off. Superseded by the Rust implementation at
the repo root — kept here as a reference/experiment, not the shipped tool.

See the [root README](../../README.md) for current status.

## Build / run

```bash
make build   # builds bin/skillscope
make run     # launches the TUI against the local transcript corpus

./bin/skillscope summary            # per-skill counts + trigger breakdown
./bin/skillscope sessions <skill>   # session drill-down for one skill
./bin/skillscope timeline [skill]   # time-series (daily/weekly)
./bin/skillscope projects           # per-project breakdown
./bin/skillscope fidelity           # trigger-fidelity report
./bin/skillscope export             # JSON export of normalized invocations
```

## Architecture

```
cmd/skillscope/       # main.go entry point
internal/
├── models/            # SkillInvocation, Origin, TriggerType
├── parser/            # JSONL streaming extraction (main + subagent)
├── aggregate/          # counts, trigger breakdown, time-series, per-project rollups
├── sessions/            # sessions-index.json join
├── fidelity/             # skill discovery + TF-IDF trigger-fidelity heuristics
├── cli/                   # subcommands + global flags
└── tui/                    # Bubbletea: skills -> sessions -> invocations drill-down
```

Does not include the session-scoped modes (`skillscope .` picker, `skillscope <session-id>`)
added to the Rust implementation after the bake-off concluded.
