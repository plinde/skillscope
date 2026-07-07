package tui

import (
	"fmt"
	"strings"
)

func (m model) View() string {
	if m.err != nil {
		return m.styles.Error.Render(fmt.Sprintf("error: %v", m.err)) + "\n"
	}

	switch m.phase {
	case phaseLoading:
		return m.styles.Title.Render("skillscope") + "\n\nLoading transcripts…\n"
	case phaseSkills:
		return m.viewSkills()
	case phaseSessions:
		return m.viewSessions()
	case phaseInvocations:
		return m.viewInvocations()
	}
	return ""
}

func (m model) renderBreadcrumb(parts ...string) string {
	return m.styles.Breadcrumb.Render(strings.Join(parts, " → "))
}

func (m model) renderFooter(help string) string {
	return "\n" + m.styles.Footer.Render(help)
}

func (m model) viewSkills() string {
	var b strings.Builder
	b.WriteString(m.styles.Title.Render("skillscope"))
	b.WriteString("\n")
	b.WriteString(m.renderBreadcrumb("skills"))
	b.WriteString(fmt.Sprintf("  [window: %s]  [sort: %s]", m.window.label(), m.sortCol.label()))
	b.WriteString("\n\n")

	if m.filtering {
		b.WriteString(m.styles.Filter.Render(m.filterText.View()))
		b.WriteString("\n\n")
	} else if v := m.filterText.Value(); v != "" {
		b.WriteString(m.styles.Filter.Render(fmt.Sprintf("filter: %q", v)))
		b.WriteString("\n\n")
	}

	b.WriteString(m.skillsTable.View())
	b.WriteString(m.renderFooter("↑/↓ move · enter drill in · / filter · s cycle sort · S reverse sort · t time window · q quit"))
	return b.String()
}

func (m model) viewSessions() string {
	var b strings.Builder
	b.WriteString(m.styles.Title.Render("skillscope"))
	b.WriteString("\n")
	b.WriteString(m.renderBreadcrumb("skills", m.selectedSkill))
	b.WriteString(fmt.Sprintf("  [window: %s]", m.window.label()))
	b.WriteString("\n\n")
	b.WriteString(m.sessionsTable.View())
	b.WriteString(m.renderFooter("↑/↓ move · enter drill in · esc back · t time window · q quit"))
	return b.String()
}

func (m model) viewInvocations() string {
	var b strings.Builder
	b.WriteString(m.styles.Title.Render("skillscope"))
	b.WriteString("\n")

	title := m.selectedSessionID
	if m.sessIdx != nil {
		title = m.sessIdx.DisplayTitle(m.selectedSessionID)
	}
	b.WriteString(m.renderBreadcrumb("skills", m.selectedSkill, title))
	b.WriteString("\n\n")

	if len(m.invocationRows) == 0 {
		b.WriteString("(no invocations)\n")
	}
	for i, inv := range m.invocationRows {
		cursor := "  "
		if i == m.invocationCursor {
			cursor = m.styles.Cursor.Render("▶ ")
		}
		args := inv.Args
		if args == "" {
			args = "(no args)"
		}
		line := fmt.Sprintf("%s%s  %-18s  trigger=%s origin=%s args=%s",
			cursor,
			inv.Timestamp.Format("2006-01-02T15:04:05"),
			inv.SkillName,
			inv.TriggerType,
			inv.Origin,
			args,
		)
		b.WriteString(line)
		b.WriteString("\n")
	}
	b.WriteString(m.renderFooter("↑/↓ move · esc back · q quit"))
	return b.String()
}
