// Package aggregate holds pure aggregation functions over
// SkillInvocation slices. No I/O here — everything takes a slice
// (typically collected from parser.IterInvocations) and returns plain
// structs. Callers filter by --since/--origin before calling in.
// Mirrors src/skillscope/aggregate.py.
package aggregate

import (
	"sort"
	"time"

	"github.com/plinde/skillscope/internal/models"
)

// SkillCount is per-skill totals, trigger-type breakdown, subagent
// origin count, and first/last-seen timestamps.
type SkillCount struct {
	SkillName       string
	Total           int
	UserSlash       int
	ClaudeProactive int
	SubagentOrigin  int // additive: invocations with Origin == subagent
	FirstSeen       time.Time
	LastSeen        time.Time
}

// SkillCounts returns per-skill totals, sorted by total descending.
func SkillCounts(invs []models.SkillInvocation) []SkillCount {
	index := map[string]*SkillCount{}
	var order []string

	for _, inv := range invs {
		entry, ok := index[inv.SkillName]
		if !ok {
			entry = &SkillCount{SkillName: inv.SkillName, FirstSeen: inv.Timestamp, LastSeen: inv.Timestamp}
			index[inv.SkillName] = entry
			order = append(order, inv.SkillName)
		}
		entry.Total++
		switch inv.TriggerType {
		case models.TriggerUserSlash:
			entry.UserSlash++
		case models.TriggerClaudeProactive:
			entry.ClaudeProactive++
		}
		if inv.Origin == models.OriginSubagent {
			entry.SubagentOrigin++
		}
		if inv.Timestamp.Before(entry.FirstSeen) {
			entry.FirstSeen = inv.Timestamp
		}
		if inv.Timestamp.After(entry.LastSeen) {
			entry.LastSeen = inv.Timestamp
		}
	}

	rows := make([]SkillCount, 0, len(order))
	for _, name := range order {
		rows = append(rows, *index[name])
	}
	sort.SliceStable(rows, func(i, j int) bool { return rows[i].Total > rows[j].Total })
	return rows
}

// SessionSummary is a session that fired a given skill, with per-session
// count and time span.
type SessionSummary struct {
	SessionID   string
	ProjectPath string
	Count       int
	FirstTS     time.Time
	LastTS      time.Time
}

// SessionsForSkill returns sessions that fired skill, sorted by count
// descending.
func SessionsForSkill(invs []models.SkillInvocation, skill string) []SessionSummary {
	index := map[string]*SessionSummary{}
	var order []string

	for _, inv := range invs {
		if inv.SkillName != skill {
			continue
		}
		entry, ok := index[inv.SessionID]
		if !ok {
			entry = &SessionSummary{SessionID: inv.SessionID, ProjectPath: inv.ProjectPath, FirstTS: inv.Timestamp, LastTS: inv.Timestamp}
			index[inv.SessionID] = entry
			order = append(order, inv.SessionID)
		}
		entry.Count++
		if inv.Timestamp.Before(entry.FirstTS) {
			entry.FirstTS = inv.Timestamp
		}
		if inv.Timestamp.After(entry.LastTS) {
			entry.LastTS = inv.Timestamp
		}
	}

	rows := make([]SessionSummary, 0, len(order))
	for _, id := range order {
		rows = append(rows, *index[id])
	}
	sort.SliceStable(rows, func(i, j int) bool { return rows[i].Count > rows[j].Count })
	return rows
}

// Granularity for timeline bucketing.
type Granularity string

const (
	GranularityDay  Granularity = "day"
	GranularityWeek Granularity = "week"
)

func periodKey(ts time.Time, granularity Granularity) string {
	if granularity == GranularityWeek {
		// ISO-ish Monday-start week, matching aggregate.py's
		// date - timedelta(days=weekday()).
		daysSinceMonday := (int(ts.Weekday()) + 6) % 7
		monday := ts.AddDate(0, 0, -daysSinceMonday)
		return monday.Format("2006-01-02")
	}
	return ts.Format("2006-01-02")
}

// TimelinePoint is one bucket in a timeline series.
type TimelinePoint struct {
	Period string
	Count  int
}

// Timeline returns an ascending-ordered period -> invocation count
// series, optionally scoped to one skill.
func Timeline(invs []models.SkillInvocation, skill string, granularity Granularity) []TimelinePoint {
	counts := map[string]int{}
	for _, inv := range invs {
		if skill != "" && inv.SkillName != skill {
			continue
		}
		counts[periodKey(inv.Timestamp, granularity)]++
	}

	periods := make([]string, 0, len(counts))
	for p := range counts {
		periods = append(periods, p)
	}
	sort.Strings(periods)

	rows := make([]TimelinePoint, 0, len(periods))
	for _, p := range periods {
		rows = append(rows, TimelinePoint{Period: p, Count: counts[p]})
	}
	return rows
}

// TopSkill is one entry in a project's top-skills list.
type TopSkill struct {
	Name  string
	Count int
}

// ProjectCount is per-project totals plus top-5 skills by count.
type ProjectCount struct {
	Project   string
	Total     int
	TopSkills []TopSkill
}

// ProjectCounts returns per-project totals, sorted by total descending.
func ProjectCounts(invs []models.SkillInvocation) []ProjectCount {
	totals := map[string]int{}
	perSkill := map[string]map[string]int{}
	var order []string

	for _, inv := range invs {
		if _, ok := totals[inv.ProjectPath]; !ok {
			order = append(order, inv.ProjectPath)
			perSkill[inv.ProjectPath] = map[string]int{}
		}
		totals[inv.ProjectPath]++
		perSkill[inv.ProjectPath][inv.SkillName]++
	}

	rows := make([]ProjectCount, 0, len(order))
	for _, project := range order {
		skillCounts := perSkill[project]
		names := make([]string, 0, len(skillCounts))
		for name := range skillCounts {
			names = append(names, name)
		}
		sort.SliceStable(names, func(i, j int) bool { return skillCounts[names[i]] > skillCounts[names[j]] })
		if len(names) > 5 {
			names = names[:5]
		}
		top := make([]TopSkill, 0, len(names))
		for _, name := range names {
			top = append(top, TopSkill{Name: name, Count: skillCounts[name]})
		}
		rows = append(rows, ProjectCount{Project: project, Total: totals[project], TopSkills: top})
	}
	sort.SliceStable(rows, func(i, j int) bool { return rows[i].Total > rows[j].Total })
	return rows
}
