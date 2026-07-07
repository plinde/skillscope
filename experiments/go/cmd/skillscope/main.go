// Command skillscope analyzes Claude Code skill/slash-command
// invocations from local JSONL transcripts. Bare invocation launches
// the interactive TUI; subcommands provide scripted/CI-friendly
// output.
package main

import (
	"fmt"
	"os"

	"github.com/plinde/skillscope/internal/cli"
	"github.com/plinde/skillscope/internal/tui"
)

const usage = `skillscope: Claude Code skill-invocation analytics (local JSONL transcripts only)

Usage:
  skillscope                          Launch the interactive TUI
  skillscope summary                  Per-skill counts and trigger breakdown
  skillscope sessions <skill>         Session drill-down for one skill
  skillscope timeline [skill]         Time-series of invocations
  skillscope projects                 Per-project skill usage breakdown
  skillscope export                   JSON-lines export of normalized invocations
  skillscope fidelity                 Trigger-fidelity report

Global flags (place before the subcommand):
  --projects-dir DIR   Directory containing Claude Code project transcripts
                        (default: ~/.claude/projects)
  --since VALUE         Only include invocations on/after this time.
                        Accepts a relative duration (7d, 30d, 12w, 6m, 1y)
                        or a literal date (YYYY-MM-DD).
  --origin ORIGIN       Filter by invocation origin: main | subagent
  --json                Emit machine-readable JSON output

Subcommand flags:
  timeline --week       Group by week instead of day
`

func main() {
	args := os.Args[1:]

	opts := cli.Options{ProjectsDir: cli.DefaultProjectsDir()}
	var rest []string

	for i := 0; i < len(args); i++ {
		switch args[i] {
		case "--projects-dir":
			i++
			if i >= len(args) {
				fatalf("--projects-dir requires a value")
			}
			opts.ProjectsDir = args[i]
		case "--since":
			i++
			if i >= len(args) {
				fatalf("--since requires a value")
			}
			t, err := cli.ParseSince(args[i])
			if err != nil {
				fatalf("%v", err)
			}
			opts.Since = &t
		case "--origin":
			i++
			if i >= len(args) {
				fatalf("--origin requires a value")
			}
			opts.Origin = args[i]
		case "--json":
			opts.JSON = true
		case "-h", "--help", "help":
			fmt.Print(usage)
			return
		default:
			rest = append(rest, args[i])
		}
	}

	if len(rest) == 0 {
		if err := tui.Run(opts); err != nil {
			fatalf("%v", err)
		}
		return
	}

	cmd, cmdArgs := rest[0], rest[1:]
	var err error
	switch cmd {
	case "summary":
		err = cli.CmdSummary(os.Stdout, opts)
	case "sessions":
		if len(cmdArgs) < 1 {
			fatalf("usage: skillscope sessions <skill>")
		}
		err = cli.CmdSessions(os.Stdout, opts, cmdArgs[0])
	case "timeline":
		week := false
		skill := ""
		for _, a := range cmdArgs {
			if a == "--week" {
				week = true
				continue
			}
			if skill == "" {
				skill = a
			}
		}
		err = cli.CmdTimeline(os.Stdout, opts, skill, week)
	case "projects":
		err = cli.CmdProjects(os.Stdout, opts)
	case "export":
		err = cli.CmdExport(os.Stdout, opts)
	case "fidelity":
		err = cli.CmdFidelity(os.Stdout, opts)
	default:
		fmt.Fprintf(os.Stderr, "unknown command: %s\n\n", cmd)
		fmt.Print(usage)
		os.Exit(1)
	}

	if err != nil {
		fatalf("%v", err)
	}
}

func fatalf(format string, args ...any) {
	fmt.Fprintf(os.Stderr, "skillscope: "+format+"\n", args...)
	os.Exit(1)
}
