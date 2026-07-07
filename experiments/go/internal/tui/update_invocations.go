package tui

import (
	"github.com/charmbracelet/bubbles/key"
	tea "github.com/charmbracelet/bubbletea"
)

func (m model) updateInvocationsPhase(msg tea.KeyMsg) (tea.Model, tea.Cmd) {
	switch {
	case key.Matches(msg, m.keys.Back):
		m.phase = phaseSessions
		return m, nil

	case key.Matches(msg, m.keys.Up):
		if m.invocationCursor > 0 {
			m.invocationCursor--
		}
		return m, nil

	case key.Matches(msg, m.keys.Down):
		if m.invocationCursor < len(m.invocationRows)-1 {
			m.invocationCursor++
		}
		return m, nil
	}
	return m, nil
}
