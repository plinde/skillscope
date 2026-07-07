package tui

import (
	"fmt"
	"os"
	"sort"
	"strings"
	"time"

	"github.com/charmbracelet/bubbles/key"
	"github.com/charmbracelet/bubbles/table"
	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"

	"github.com/plinde/skillscope/internal/aggregate"
	"github.com/plinde/skillscope/internal/cli"
	"github.com/plinde/skillscope/internal/models"
	"github.com/plinde/skillscope/internal/sessions"
)

type phase int

const (
	phaseLoading phase = iota
	phaseSkills
	phaseSessions
	phaseInvocations
)

type sortColumn int

const (
	sortByTotal sortColumn = iota
	sortBySlash
	sortByProactive
	sortBySubagent
	sortByLastSeen
)

func (s sortColumn) label() string {
	switch s {
	case sortByTotal:
		return "total"
	case sortBySlash:
		return "slash"
	case sortByProactive:
		return "proactive"
	case sortBySubagent:
		return "subagent"
	case sortByLastSeen:
		return "last-seen"
	default:
		return "?"
	}
}

// timeWindow cycles 7d -> 30d -> all.
type timeWindow int

const (
	window7d timeWindow = iota
	window30d
	windowAll
)

func (w timeWindow) label() string {
	switch w {
	case window7d:
		return "7d"
	case window30d:
		return "30d"
	default:
		return "all"
	}
}

func (w timeWindow) cutoff() *time.Time {
	now := time.Now().UTC()
	var t time.Time
	switch w {
	case window7d:
		t = now.AddDate(0, 0, -7)
	case window30d:
		t = now.AddDate(0, 0, -30)
	default:
		return nil
	}
	return &t
}

type model struct {
	keys   keymap
	styles styles

	width  int
	height int

	phase phase
	err   error

	loadOpts cli.Options
	allInvs  []models.SkillInvocation
	sessIdx  *sessions.Index

	window     timeWindow
	sortCol    sortColumn
	sortDesc   bool
	filtering  bool
	filterText textinput.Model

	skillsTable         table.Model
	filteredSkillCounts []aggregate.SkillCount

	selectedSkill    string
	sessionsTable    table.Model
	filteredSessions []aggregate.SessionSummary

	selectedSessionID string
	invocationRows    []models.SkillInvocation
	invocationCursor  int
}

// New builds the initial model. opts.Since/Origin are ignored for the
// TUI's own filtering (the TUI applies its own time-window toggle and
// has no origin toggle yet); ProjectsDir is honored.
func New(opts cli.Options) model {
	ti := textinput.New()
	ti.Placeholder = "filter skills..."
	ti.CharLimit = 64
	ti.Prompt = "/ "

	loadOpts := opts
	loadOpts.Since = nil
	loadOpts.Origin = ""

	return model{
		keys:        newKeymap(),
		styles:      newStyles(),
		phase:       phaseLoading,
		loadOpts:    loadOpts,
		window:      windowAll,
		sortCol:     sortByTotal,
		sortDesc:    true,
		filterText:  ti,
		skillsTable: table.New(table.WithFocused(true)),
	}
}

func (m model) Init() tea.Cmd {
	return loadInvocationsCmd(m.loadOpts)
}

func loadInvocationsCmd(opts cli.Options) tea.Cmd {
	return func() tea.Msg {
		invs, err := cli.LoadInvocations(opts)
		return invocationsLoadedMsg{invocations: invs, err: err}
	}
}

// Run launches the TUI program.
func Run(opts cli.Options) error {
	m := New(opts)
	p := tea.NewProgram(m, tea.WithAltScreen())
	_, err := p.Run()
	return err
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width = msg.Width
		m.height = msg.Height
		m.applyTableSizes()
		return m, nil

	case invocationsLoadedMsg:
		if msg.err != nil {
			m.err = msg.err
			return m, nil
		}
		m.allInvs = msg.invocations
		home, _ := os.UserHomeDir()
		idx, err := sessions.Load(home + "/.claude/projects")
		if err == nil {
			m.sessIdx = idx
		}
		m.phase = phaseSkills
		m.rebuildSkillsTable()
		return m, nil

	case tea.KeyMsg:
		return m.handleKey(msg)
	}
	return m, nil
}

func (m *model) applyTableSizes() {
	h := m.height - 6
	if h < 3 {
		h = 3
	}
	m.skillsTable.SetHeight(h)
	m.sessionsTable.SetHeight(h)
	w := m.width
	if w < 20 {
		w = 20
	}
	m.skillsTable.SetWidth(w)
	m.sessionsTable.SetWidth(w)
}

func (m model) windowedInvs() []models.SkillInvocation {
	cutoff := m.window.cutoff()
	if cutoff == nil {
		return m.allInvs
	}
	var out []models.SkillInvocation
	for _, inv := range m.allInvs {
		if !inv.Timestamp.Before(*cutoff) {
			out = append(out, inv)
		}
	}
	return out
}

func (m *model) rebuildSkillsTable() {
	counts := aggregate.SkillCounts(m.windowedInvs())

	filter := strings.ToLower(m.filterText.Value())
	var filtered []aggregate.SkillCount
	for _, c := range counts {
		if filter == "" || strings.Contains(strings.ToLower(c.SkillName), filter) {
			filtered = append(filtered, c)
		}
	}

	sort.SliceStable(filtered, func(i, j int) bool {
		var less bool
		switch m.sortCol {
		case sortByTotal:
			less = filtered[i].Total < filtered[j].Total
		case sortBySlash:
			less = filtered[i].UserSlash < filtered[j].UserSlash
		case sortByProactive:
			less = filtered[i].ClaudeProactive < filtered[j].ClaudeProactive
		case sortBySubagent:
			less = filtered[i].SubagentOrigin < filtered[j].SubagentOrigin
		case sortByLastSeen:
			less = filtered[i].LastSeen.Before(filtered[j].LastSeen)
		}
		if m.sortDesc {
			return !less && filtered[i].SkillName != filtered[j].SkillName
		}
		return less
	})

	m.filteredSkillCounts = filtered

	cols := []table.Column{
		{Title: "Skill", Width: 30},
		{Title: sortIndicator("Total", m.sortCol == sortByTotal, m.sortDesc), Width: 8},
		{Title: sortIndicator("Slash", m.sortCol == sortBySlash, m.sortDesc), Width: 8},
		{Title: sortIndicator("Proactive", m.sortCol == sortByProactive, m.sortDesc), Width: 10},
		{Title: sortIndicator("Subagent", m.sortCol == sortBySubagent, m.sortDesc), Width: 9},
		{Title: sortIndicator("Last seen", m.sortCol == sortByLastSeen, m.sortDesc), Width: 12},
	}
	rows := make([]table.Row, 0, len(filtered))
	for _, c := range filtered {
		rows = append(rows, table.Row{
			c.SkillName,
			fmt.Sprintf("%d", c.Total),
			fmt.Sprintf("%d", c.UserSlash),
			fmt.Sprintf("%d", c.ClaudeProactive),
			fmt.Sprintf("%d", c.SubagentOrigin),
			c.LastSeen.Format("2006-01-02"),
		})
	}
	m.skillsTable.SetColumns(cols)
	m.skillsTable.SetRows(rows)
	m.applyTableSizes()
}

func sortIndicator(label string, active, desc bool) string {
	if !active {
		return label
	}
	if desc {
		return label + " ▼"
	}
	return label + " ▲"
}

func (m *model) rebuildSessionsTable() {
	rows := aggregate.SessionsForSkill(m.windowedInvs(), m.selectedSkill)
	m.filteredSessions = rows

	cols := []table.Column{
		{Title: "Session", Width: 40},
		{Title: "Branch", Width: 20},
		{Title: "Count", Width: 6},
		{Title: "First", Width: 17},
		{Title: "Last", Width: 17},
	}
	tableRows := make([]table.Row, 0, len(rows))
	for _, r := range rows {
		title := r.SessionID
		branch := ""
		if m.sessIdx != nil {
			title = m.sessIdx.DisplayTitle(r.SessionID)
			branch = m.sessIdx.GitBranch(r.SessionID)
		}
		tableRows = append(tableRows, table.Row{
			title,
			branch,
			fmt.Sprintf("%d", r.Count),
			r.FirstTS.Format("2006-01-02T15:04"),
			r.LastTS.Format("2006-01-02T15:04"),
		})
	}
	m.sessionsTable.SetColumns(cols)
	m.sessionsTable.SetRows(tableRows)
	m.applyTableSizes()
}

func (m *model) rebuildInvocationRows() {
	var rows []models.SkillInvocation
	for _, inv := range m.windowedInvs() {
		if inv.SkillName == m.selectedSkill && inv.SessionID == m.selectedSessionID {
			rows = append(rows, inv)
		}
	}
	sort.SliceStable(rows, func(i, j int) bool { return rows[i].Timestamp.Before(rows[j].Timestamp) })
	m.invocationRows = rows
	m.invocationCursor = 0
}

func (m model) handleKey(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	if m.filtering {
		switch {
		case msg.String() == "esc":
			m.filtering = false
			m.filterText.Blur()
			return m, nil
		case msg.String() == "enter":
			m.filtering = false
			m.filterText.Blur()
			m.rebuildSkillsTable()
			return m, nil
		default:
			var cmd tea.Cmd
			m.filterText, cmd = m.filterText.Update(msg)
			m.rebuildSkillsTable()
			return m, cmd
		}
	}

	switch {
	case key.Matches(msg, m.keys.Quit):
		return m, tea.Quit
	}

	switch m.phase {
	case phaseSkills:
		return m.updateSkillsPhase(msg)
	case phaseSessions:
		return m.updateSessionsPhase(msg)
	case phaseInvocations:
		return m.updateInvocationsPhase(msg)
	}
	return m, nil
}
