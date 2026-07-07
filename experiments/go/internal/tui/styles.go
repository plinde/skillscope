package tui

import "github.com/charmbracelet/lipgloss"

// styles is the "lipgloss" default ANSI-256 skin (per tui-bubbletea
// SKILL.md's two documented palettes; the warm/terracotta "claude"
// skin is left as a documented future option, not wired to a flag
// here since the spec didn't call for skin switching).
type styles struct {
	Title      lipgloss.Style
	Breadcrumb lipgloss.Style
	Footer     lipgloss.Style
	HelpKey    lipgloss.Style
	HelpDesc   lipgloss.Style
	Cursor     lipgloss.Style
	Header     lipgloss.Style
	SortArrow  lipgloss.Style
	Filter     lipgloss.Style
	Error      lipgloss.Style
}

func newStyles() styles {
	return styles{
		Title: lipgloss.NewStyle().
			Bold(true).
			Foreground(lipgloss.Color("39")).
			Padding(0, 1),
		Breadcrumb: lipgloss.NewStyle().
			Foreground(lipgloss.Color("245")),
		Footer: lipgloss.NewStyle().
			Foreground(lipgloss.Color("240")),
		HelpKey: lipgloss.NewStyle().
			Foreground(lipgloss.Color("214")).
			Bold(true),
		HelpDesc: lipgloss.NewStyle().
			Foreground(lipgloss.Color("245")),
		Cursor: lipgloss.NewStyle().
			Foreground(lipgloss.Color("39")).
			Bold(true),
		Header: lipgloss.NewStyle().
			Bold(true).
			Foreground(lipgloss.Color("250")),
		SortArrow: lipgloss.NewStyle().
			Foreground(lipgloss.Color("214")),
		Filter: lipgloss.NewStyle().
			Foreground(lipgloss.Color("214")),
		Error: lipgloss.NewStyle().
			Foreground(lipgloss.Color("196")).
			Bold(true),
	}
}
