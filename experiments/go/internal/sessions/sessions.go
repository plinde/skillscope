// Package sessions reads ~/.claude/projects/*/sessions-index.json and
// joins entries onto SkillInvocation.SessionID. The index may be stale
// or absent for a project — every lookup degrades gracefully to the
// raw UUID when no entry is found.
package sessions

import (
	"encoding/json"
	"os"
	"path/filepath"
)

// Entry is one session-index record.
type Entry struct {
	SessionID    string `json:"sessionId"`
	FullPath     string `json:"fullPath"`
	FirstPrompt  string `json:"firstPrompt"`
	Summary      string `json:"summary"`
	MessageCount int    `json:"messageCount"`
	Created      string `json:"created"`
	Modified     string `json:"modified"`
	GitBranch    string `json:"gitBranch"`
	ProjectPath  string `json:"projectPath"`
	IsSidechain  bool   `json:"isSidechain"`
}

type indexFile struct {
	Version      int     `json:"version"`
	OriginalPath string  `json:"originalPath"`
	Entries      []Entry `json:"entries"`
}

// Index joins sessionId -> Entry across every project's
// sessions-index.json under projectsDir.
type Index struct {
	bySessionID map[string]Entry
}

// Load reads every sessions-index.json under projectsDir. Missing or
// malformed index files are skipped silently — the index is a
// best-effort join aid, not authoritative data.
func Load(projectsDir string) (*Index, error) {
	idx := &Index{bySessionID: map[string]Entry{}}

	paths, err := filepath.Glob(filepath.Join(projectsDir, "*", "sessions-index.json"))
	if err != nil {
		return idx, err
	}
	for _, p := range paths {
		data, err := os.ReadFile(p)
		if err != nil {
			continue
		}
		var f indexFile
		if err := json.Unmarshal(data, &f); err != nil {
			continue
		}
		for _, e := range f.Entries {
			if e.SessionID == "" {
				continue
			}
			idx.bySessionID[e.SessionID] = e
		}
	}
	return idx, nil
}

// Lookup returns the indexed entry for sessionID, if any.
func (idx *Index) Lookup(sessionID string) (Entry, bool) {
	if idx == nil {
		return Entry{}, false
	}
	e, ok := idx.bySessionID[sessionID]
	return e, ok
}

// DisplayTitle returns the best available human label for a session:
// summary, falling back to a truncated firstPrompt, falling back to
// the raw session UUID.
func (idx *Index) DisplayTitle(sessionID string) string {
	e, ok := idx.Lookup(sessionID)
	if !ok {
		return sessionID
	}
	if e.Summary != "" {
		return e.Summary
	}
	if e.FirstPrompt != "" {
		p := e.FirstPrompt
		const maxLen = 80
		if len(p) > maxLen {
			p = p[:maxLen] + "…"
		}
		return p
	}
	return sessionID
}

// GitBranch returns the indexed git branch for sessionID, or "" if
// there's no entry.
func (idx *Index) GitBranch(sessionID string) string {
	e, ok := idx.Lookup(sessionID)
	if !ok {
		return ""
	}
	return e.GitBranch
}
