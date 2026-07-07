// Package cli implements skillscope's subcommand layer: summary,
// sessions, timeline, projects, export, fidelity. Bare invocation (no
// subcommand) launches the TUI instead — see cmd/skillscope/main.go.
package cli

import (
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/plinde/skillscope/internal/aggregate"
	"github.com/plinde/skillscope/internal/fidelity"
	"github.com/plinde/skillscope/internal/models"
	"github.com/plinde/skillscope/internal/parser"
)

// DefaultProjectsDir mirrors cli.py's DEFAULT_PROJECTS_DIR.
func DefaultProjectsDir() string {
	home, err := os.UserHomeDir()
	if err != nil {
		return ""
	}
	return filepath.Join(home, ".claude", "projects")
}

// Options are the global flags shared by every subcommand.
type Options struct {
	ProjectsDir string
	Since       *time.Time // parsed from --since; nil means no cutoff
	JSON        bool
	Origin      string // "", "main", or "subagent" — Feature 2 filter
}

var sinceRelativeRe = regexp.MustCompile(`^(\d+)([dwmy])$`)

// ParseSince accepts either a relative duration string (7d, 30d, 12w,
// 6m, 1y) — the Go CLI's departure from the Python's YYYY-MM-DD-only
// format — or a literal YYYY-MM-DD date, for compatibility with the
// original format.
func ParseSince(value string) (time.Time, error) {
	if m := sinceRelativeRe.FindStringSubmatch(value); m != nil {
		n, _ := strconv.Atoi(m[1])
		now := time.Now().UTC()
		switch m[2] {
		case "d":
			return now.AddDate(0, 0, -n), nil
		case "w":
			return now.AddDate(0, 0, -n*7), nil
		case "m":
			return now.AddDate(0, -n, 0), nil
		case "y":
			return now.AddDate(-n, 0, 0), nil
		}
	}
	t, err := time.Parse("2006-01-02", value)
	if err != nil {
		return time.Time{}, fmt.Errorf("--since must be a relative duration (7d, 30d, 12w, 6m, 1y) or YYYY-MM-DD, got %q", value)
	}
	return t.UTC(), nil
}

func matchesOrigin(inv models.SkillInvocation, origin string) bool {
	switch origin {
	case "", "all":
		return true
	case string(models.OriginMain):
		return inv.Origin == models.OriginMain
	case string(models.OriginSubagent):
		return inv.Origin == models.OriginSubagent
	default:
		return true
	}
}

// LoadInvocations streams IterInvocations, applying the --since and
// --origin filters, and materializes the result. Mirrors cli.py's
// _load_invocations, extended for Feature 2.
func LoadInvocations(opts Options) ([]models.SkillInvocation, error) {
	var invs []models.SkillInvocation
	err := parser.IterInvocations(opts.ProjectsDir, func(inv models.SkillInvocation) bool {
		if opts.Since != nil && inv.Timestamp.Before(*opts.Since) {
			return true
		}
		if !matchesOrigin(inv, opts.Origin) {
			return true
		}
		invs = append(invs, inv)
		return true
	})
	return invs, err
}

func printJSON(w io.Writer, v any) {
	enc := json.NewEncoder(w)
	enc.SetIndent("", "  ")
	_ = enc.Encode(v)
}

// invocationJSON mirrors cli.py's _inv_to_dict field naming.
type invocationJSON struct {
	SkillName      string `json:"skill_name"`
	TriggerType    string `json:"trigger_type"`
	SessionID      string `json:"session_id"`
	ProjectPath    string `json:"project_path"`
	Timestamp      string `json:"timestamp"`
	TranscriptFile string `json:"transcript_file"`
	Args           string `json:"args,omitempty"`
	Origin         string `json:"origin"`
}

func invToJSON(inv models.SkillInvocation) invocationJSON {
	return invocationJSON{
		SkillName:      inv.SkillName,
		TriggerType:    string(inv.TriggerType),
		SessionID:      inv.SessionID,
		ProjectPath:    inv.ProjectPath,
		Timestamp:      inv.Timestamp.Format(time.RFC3339),
		TranscriptFile: inv.TranscriptFile,
		Args:           inv.Args,
		Origin:         string(inv.Origin),
	}
}

func padRight(s string, width int) string {
	if len(s) >= width {
		return s
	}
	return s + strings.Repeat(" ", width-len(s))
}

func printTable(out io.Writer, title string, headers []string, widths []int, rows [][]string) {
	fmt.Fprintln(out, title)
	headerCells := make([]string, len(headers))
	for i, h := range headers {
		headerCells[i] = padRight(h, widths[i])
	}
	fmt.Fprintln(out, strings.Join(headerCells, "  "))
	sepCells := make([]string, len(headers))
	for i := range headers {
		sepCells[i] = strings.Repeat("-", widths[i])
	}
	fmt.Fprintln(out, strings.Join(sepCells, "  "))
	for _, row := range rows {
		cells := make([]string, len(row))
		for i, c := range row {
			colWidth := 0
			if i < len(widths) {
				colWidth = widths[i]
			}
			cells[i] = padRight(c, colWidth)
		}
		fmt.Fprintln(out, strings.Join(cells, "  "))
	}
}

// CmdSummary implements `skillscope summary`.
func CmdSummary(w io.Writer, opts Options) error {
	invs, err := LoadInvocations(opts)
	if err != nil {
		return err
	}
	counts := aggregate.SkillCounts(invs)

	if opts.JSON {
		printJSON(w, counts)
		return nil
	}

	headers := []string{"Skill", "Total", "User /slash", "Claude proactive", "Subagent", "First seen", "Last seen"}
	widths := []int{30, 6, 12, 17, 9, 12, 12}
	rows := make([][]string, 0, len(counts))
	for _, c := range counts {
		rows = append(rows, []string{
			c.SkillName,
			strconv.Itoa(c.Total),
			strconv.Itoa(c.UserSlash),
			strconv.Itoa(c.ClaudeProactive),
			strconv.Itoa(c.SubagentOrigin),
			c.FirstSeen.Format("2006-01-02"),
			c.LastSeen.Format("2006-01-02"),
		})
	}
	printTable(w, "Skill invocation summary", headers, widths, rows)
	return nil
}

// CmdSessions implements `skillscope sessions <skill>`.
func CmdSessions(w io.Writer, opts Options, skill string) error {
	invs, err := LoadInvocations(opts)
	if err != nil {
		return err
	}
	rows := aggregate.SessionsForSkill(invs, skill)

	if opts.JSON {
		printJSON(w, rows)
		return nil
	}

	headers := []string{"Session ID", "Project", "Count", "First", "Last"}
	widths := []int{36, 30, 6, 17, 17}
	tableRows := make([][]string, 0, len(rows))
	for _, r := range rows {
		tableRows = append(tableRows, []string{
			r.SessionID,
			r.ProjectPath,
			strconv.Itoa(r.Count),
			r.FirstTS.Format("2006-01-02T15:04"),
			r.LastTS.Format("2006-01-02T15:04"),
		})
	}
	printTable(w, fmt.Sprintf("Sessions invoking '%s'", skill), headers, widths, tableRows)
	return nil
}

// CmdTimeline implements `skillscope timeline [skill] [--week]`.
func CmdTimeline(w io.Writer, opts Options, skill string, week bool) error {
	invs, err := LoadInvocations(opts)
	if err != nil {
		return err
	}
	granularity := aggregate.GranularityDay
	if week {
		granularity = aggregate.GranularityWeek
	}
	series := aggregate.Timeline(invs, skill, granularity)

	if opts.JSON {
		printJSON(w, series)
		return nil
	}

	title := fmt.Sprintf("Timeline (%s)", granularity)
	if skill != "" {
		title += fmt.Sprintf(" for '%s'", skill)
	}
	headers := []string{"Period", "Count"}
	widths := []int{12, 6}
	rows := make([][]string, 0, len(series))
	for _, p := range series {
		rows = append(rows, []string{p.Period, strconv.Itoa(p.Count)})
	}
	printTable(w, title, headers, widths, rows)
	return nil
}

// CmdProjects implements `skillscope projects`.
func CmdProjects(w io.Writer, opts Options) error {
	invs, err := LoadInvocations(opts)
	if err != nil {
		return err
	}
	counts := aggregate.ProjectCounts(invs)

	if opts.JSON {
		printJSON(w, counts)
		return nil
	}

	headers := []string{"Project", "Total", "Top skills"}
	widths := []int{40, 6, 60}
	rows := make([][]string, 0, len(counts))
	for _, c := range counts {
		top := make([]string, 0, len(c.TopSkills))
		for _, s := range c.TopSkills {
			top = append(top, fmt.Sprintf("%s (%d)", s.Name, s.Count))
		}
		rows = append(rows, []string{c.Project, strconv.Itoa(c.Total), strings.Join(top, ", ")})
	}
	printTable(w, "Per-project skill usage", headers, widths, rows)
	return nil
}

// CmdExport implements `skillscope export`: newline-delimited JSON of
// normalized invocations, one per line, matching cli.py's cmd_export.
func CmdExport(w io.Writer, opts Options) error {
	invs, err := LoadInvocations(opts)
	if err != nil {
		return err
	}
	enc := json.NewEncoder(w)
	for _, inv := range invs {
		if err := enc.Encode(invToJSON(inv)); err != nil {
			return err
		}
	}
	return nil
}

// CmdFidelity implements `skillscope fidelity`.
func CmdFidelity(w io.Writer, opts Options) error {
	home, _ := os.UserHomeDir()
	fopts := fidelity.Options{
		ProjectsDir:           opts.ProjectsDir,
		SkillsDirs:            fidelity.DefaultSkillsDirs(home),
		PluginMarketplacesDir: fidelity.PluginMarketplacesDir(home),
	}
	if opts.Since != nil {
		fopts.Since = fidelity.NewSinceFilter(opts.Since.UnixNano())
	}
	report, err := fidelity.Run(fopts)
	if err != nil {
		return err
	}

	if opts.JSON {
		printJSON(w, report)
		return nil
	}

	printFidelitySection(w, "Under-triggered skills (matched intent, never fired)", report.UnderTriggered)
	printFidelitySection(w, "Over-triggered skills (fired on unrelated prompts)", report.OverTriggered)
	return nil
}

func printFidelitySection(w io.Writer, title string, findings []fidelity.Finding) {
	sort.SliceStable(findings, func(i, j int) bool { return findings[i].Count > findings[j].Count })
	headers := []string{"Skill", "Count", "Evidence"}
	widths := []int{30, 6, 80}
	rows := make([][]string, 0, len(findings))
	for _, f := range findings {
		rows = append(rows, []string{f.SkillName, strconv.Itoa(f.Count), f.Evidence})
	}
	printTable(w, title, headers, widths, rows)
}
