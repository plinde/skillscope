package tui

import (
	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
)

func (m model) updateSkillsPhase(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch {
	case key.Matches(msg, m.keys.Filter):
		m.filtering = true
		m.filterText.Focus()
		return m, nil

	case key.Matches(msg, m.keys.Window):
		m.window = (m.window + 1) % 3
		m.rebuildSkillsTable()
		return m, nil

	case key.Matches(msg, m.keys.Sort):
		m.sortCol = (m.sortCol + 1) % 5
		m.rebuildSkillsTable()
		return m, nil

	case msg.String() == "S":
		m.sortDesc = !m.sortDesc
		m.rebuildSkillsTable()
		return m, nil

	case key.Matches(msg, m.keys.Enter):
		idx := m.skillsTable.Cursor()
		if idx >= 0 && idx < len(m.filteredSkillCounts) {
			m.selectedSkill = m.filteredSkillCounts[idx].SkillName
			m.phase = phaseSessions
			m.rebuildSessionsTable()
		}
		return m, nil

	default:
		var cmd tea.Cmd
		m.skillsTable, cmd = m.skillsTable.Update(msg)
		return m, cmd
	}
}
