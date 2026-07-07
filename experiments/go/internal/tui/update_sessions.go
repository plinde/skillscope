package tui

import (
	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
)

func (m model) updateSessionsPhase(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch {
	case key.Matches(msg, m.keys.Back):
		m.phase = phaseSkills
		return m, nil

	case key.Matches(msg, m.keys.Window):
		m.window = (m.window + 1) % 3
		m.rebuildSessionsTable()
		return m, nil

	case key.Matches(msg, m.keys.Enter):
		idx := m.sessionsTable.Cursor()
		if idx >= 0 && idx < len(m.filteredSessions) {
			m.selectedSessionID = m.filteredSessions[idx].SessionID
			m.phase = phaseInvocations
			m.rebuildInvocationRows()
		}
		return m, nil

	default:
		var cmd tea.Cmd
		m.sessionsTable, cmd = m.sessionsTable.Update(msg)
		return m, cmd
	}
}
