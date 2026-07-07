// Package models defines the shared data model for skill invocations
// extracted from Claude Code JSONL transcripts. This is the contract
// between parser, aggregate, fidelity, and tui packages — it mirrors
// src/skillscope/models.py in the Python reference implementation.
package models

import "time"

// TriggerType identifies how a skill invocation was fired.
type TriggerType string

const (
	// TriggerUserSlash is a <command-name>/foo</command-name> in a
	// type:"user" transcript line.
	TriggerUserSlash TriggerType = "user-slash"
	// TriggerClaudeProactive is a tool_use with name:"Skill" in a
	// type:"assistant" transcript line.
	TriggerClaudeProactive TriggerType = "claude-proactive"
)

// Origin identifies whether an invocation came from a main-session
// transcript or a subagent transcript nested under subagents/.
type Origin string

const (
	OriginMain     Origin = "main"
	OriginSubagent Origin = "subagent"
)

// SkillInvocation is one recorded skill/slash-command firing.
type SkillInvocation struct {
	SkillName      string
	TriggerType    TriggerType
	SessionID      string
	ProjectPath    string // decoded cwd from the transcript line (or project dir name)
	Timestamp      time.Time
	TranscriptFile string
	Args           string // empty string == no args
	Origin         Origin
}

// SkillDefinition is a skill discovered on disk, for the fidelity layer.
type SkillDefinition struct {
	Name        string
	Description string // frontmatter description == the trigger heuristic
	Path        string
	Source      string // user | project | plugin
}

// UserPrompt is a real user prompt from a transcript, for fidelity
// classification.
type UserPrompt struct {
	Text          string
	SessionID     string
	ProjectPath   string
	Timestamp     time.Time
	InvokedSkills map[string]struct{} // skills actually invoked in the session
}
