// Package parser streams skill invocations and user prompts out of
// Claude Code JSONL transcripts. It mirrors src/skillscope/parser.py:
// ~/.claude/projects/*/*.jsonl transcripts are read line-by-line so the
// ~700MB corpus is never fully materialized in memory. Malformed lines
// are skipped silently — transcripts are append-only logs written by a
// live process and partial/corrupt trailing lines are expected.
package parser

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/plinde/skillscope/internal/models"
)

// ExcludedCommands are CLI built-ins, not skills — excluded from
// user-slash extraction. Copied verbatim from parser.py.
var ExcludedCommands = map[string]struct{}{
	"clear": {}, "model": {}, "help": {}, "config": {}, "compact": {}, "exit": {}, "login": {}, "logout": {},
	"status": {}, "cost": {}, "doctor": {}, "init": {}, "memory": {}, "export": {}, "resume": {}, "tasks": {},
	"agents": {}, "mcp": {}, "hooks": {}, "permissions": {}, "terminal-setup": {}, "vim": {}, "bug": {},
	"release-notes": {}, "upgrade": {}, "usage": {}, "todos": {},
}

var (
	commandNameRe = regexp.MustCompile(`<command-name>\s*/?([^<\s]+)\s*</command-name>`)
	commandArgsRe = regexp.MustCompile(`<command-args>([^<]*)</command-args>`)
)

// rawLine is the subset of transcript-line fields the parser cares
// about. message.content is deliberately left as json.RawMessage since
// its shape depends on line type (string for user, list for assistant).
type rawLine struct {
	SessionID               string          `json:"sessionId"`
	Timestamp               string          `json:"timestamp"`
	Type                    string          `json:"type"`
	Cwd                     string          `json:"cwd"`
	Message                 *rawMessage     `json:"message"`
	IsMeta                  bool            `json:"isMeta"`
	IsSidechain             bool            `json:"isSidechain"`
	ToolUseResult           json.RawMessage `json:"toolUseResult"`
	SourceToolAssistantUUID string          `json:"sourceToolAssistantUUID"`
	PromptSource            string          `json:"promptSource"`
}

type rawMessage struct {
	Content json.RawMessage `json:"content"`
}

type assistantContentEntry struct {
	Type  string          `json:"type"`
	Name  string          `json:"name"`
	Input json.RawMessage `json:"input"`
	Text  string          `json:"text"`
}

type skillToolInput struct {
	Skill   string `json:"skill"`
	Command string `json:"command"`
	Args    string `json:"args"`
}

func parseTimestamp(raw string) (time.Time, bool) {
	t, err := time.Parse(time.RFC3339Nano, raw)
	if err != nil {
		t, err = time.Parse(time.RFC3339, raw)
		if err != nil {
			return time.Time{}, false
		}
	}
	return t, true
}

// decodeProjectDir is a best-effort decode of a dashes-encoded project
// directory name into a path. The encoding is lossy (real path
// components can themselves contain dashes), so this is a fallback
// only — used when a transcript line has no cwd of its own.
func decodeProjectDir(dirName string) string {
	decoded := strings.ReplaceAll(dirName, "-", "/")
	if !strings.HasPrefix(decoded, "/") {
		decoded = "/" + decoded
	}
	return decoded
}

func loadLine(line []byte) (*rawLine, bool) {
	line = []byte(strings.TrimSpace(string(line)))
	if len(line) == 0 {
		return nil, false
	}
	var data rawLine
	if err := json.Unmarshal(line, &data); err != nil {
		return nil, false
	}
	return &data, true
}

// jsonlFiles returns every transcript path to walk for invocations:
// main-session files at <projectsDir>/*/*.jsonl, plus subagent
// transcripts at <projectsDir>/*/<session-uuid>/subagents/agent-*.jsonl
// (depths vary — glob recursively via filepath.Glob per level, since
// Go's filepath.Glob has no "**" support).
type transcriptFile struct {
	path   string
	origin models.Origin
	// fallbackProjectPath is derived from the *project* directory name
	// (the -Users-... encoded segment), not any subagent subdirectory,
	// matching the Python's per-project fallback.
	fallbackProjectPath string
}

func listTranscripts(projectsDir string) ([]transcriptFile, error) {
	var files []transcriptFile

	projectDirs, err := filepath.Glob(filepath.Join(projectsDir, "*"))
	if err != nil {
		return nil, err
	}
	for _, pdir := range projectDirs {
		info, err := os.Stat(pdir)
		if err != nil || !info.IsDir() {
			continue
		}
		fallback := decodeProjectDir(filepath.Base(pdir))

		mainFiles, err := filepath.Glob(filepath.Join(pdir, "*.jsonl"))
		if err == nil {
			for _, f := range mainFiles {
				files = append(files, transcriptFile{path: f, origin: models.OriginMain, fallbackProjectPath: fallback})
			}
		}

		subFiles, err := findSubagentFiles(pdir)
		if err == nil {
			for _, f := range subFiles {
				files = append(files, transcriptFile{path: f, origin: models.OriginSubagent, fallbackProjectPath: fallback})
			}
		}
	}
	return files, nil
}

// findSubagentFiles recurses under pdir looking for any "subagents"
// directory (at any depth — session-uuid/subagents is common, but
// depths vary per the corpus) and collects agent-*.jsonl within it.
func findSubagentFiles(pdir string) ([]string, error) {
	var found []string
	err := filepath.Walk(pdir, func(path string, info os.FileInfo, err error) error {
		if err != nil {
			return nil //nolint:nilerr // best-effort walk; skip unreadable entries
		}
		if info.IsDir() && info.Name() == "subagents" {
			matches, _ := filepath.Glob(filepath.Join(path, "agent-*.jsonl"))
			found = append(found, matches...)
		}
		return nil
	})
	return found, err
}

// IterInvocations streams SkillInvocation records from every transcript
// under projectsDir (main-session and subagent), calling yield for each.
// Returning false from yield stops iteration early.
func IterInvocations(projectsDir string, yield func(models.SkillInvocation) bool) error {
	files, err := listTranscripts(projectsDir)
	if err != nil {
		return err
	}
	for _, tf := range files {
		if !scanInvocations(tf, yield) {
			return nil
		}
	}
	return nil
}

func scanInvocations(tf transcriptFile, yield func(models.SkillInvocation) bool) bool {
	f, err := os.Open(tf.path)
	if err != nil {
		return true
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 0, 1024*1024), 64*1024*1024)
	for scanner.Scan() {
		data, ok := loadLine(scanner.Bytes())
		if !ok {
			continue
		}
		if data.SessionID == "" || data.Timestamp == "" {
			continue
		}
		ts, ok := parseTimestamp(data.Timestamp)
		if !ok {
			continue
		}
		if data.Message == nil {
			continue
		}

		projectPath := data.Cwd
		if projectPath == "" {
			projectPath = tf.fallbackProjectPath
		}

		switch data.Type {
		case "user":
			var content string
			if err := json.Unmarshal(data.Message.Content, &content); err != nil {
				continue // content is not a string (e.g. list) — not a slash invocation
			}
			nameMatch := commandNameRe.FindStringSubmatch(content)
			if nameMatch == nil {
				continue
			}
			skillName := strings.TrimSpace(nameMatch[1])
			if skillName == "" {
				continue
			}
			if _, excluded := ExcludedCommands[strings.ToLower(skillName)]; excluded {
				continue
			}
			args := ""
			if argsMatch := commandArgsRe.FindStringSubmatch(content); argsMatch != nil {
				if trimmed := strings.TrimSpace(argsMatch[1]); trimmed != "" {
					args = trimmed
				}
			}
			inv := models.SkillInvocation{
				SkillName:      skillName,
				TriggerType:    models.TriggerUserSlash,
				SessionID:      data.SessionID,
				ProjectPath:    projectPath,
				Timestamp:      ts,
				TranscriptFile: tf.path,
				Args:           args,
				Origin:         tf.origin,
			}
			if !yield(inv) {
				return false
			}

		case "assistant":
			var content []assistantContentEntry
			if err := json.Unmarshal(data.Message.Content, &content); err != nil {
				continue // content is not a list — not an assistant tool_use payload
			}
			for _, entry := range content {
				if entry.Type != "tool_use" || entry.Name != "Skill" {
					continue
				}
				var input skillToolInput
				if len(entry.Input) == 0 {
					continue
				}
				if err := json.Unmarshal(entry.Input, &input); err != nil {
					continue
				}
				skillName := input.Skill
				if skillName == "" {
					skillName = input.Command
				}
				if skillName == "" {
					continue
				}
				inv := models.SkillInvocation{
					SkillName:      skillName,
					TriggerType:    models.TriggerClaudeProactive,
					SessionID:      data.SessionID,
					ProjectPath:    projectPath,
					Timestamp:      ts,
					TranscriptFile: tf.path,
					Args:           input.Args,
					Origin:         tf.origin,
				}
				if !yield(inv) {
					return false
				}
			}
		}
	}
	return true
}

// extractPromptText mirrors parser.py's _extract_prompt_text: returns
// the free-text prompt body, or "" if this content doesn't look like a
// real user-authored prompt (XML-tag-prefixed content, e.g. tool
// results or command wrappers, and very short strings are excluded).
func extractPromptText(raw json.RawMessage) (string, bool) {
	var asString string
	if err := json.Unmarshal(raw, &asString); err == nil {
		if !strings.HasPrefix(asString, "<") && len(asString) > 10 {
			return asString, true
		}
		return "", false
	}

	var asList []assistantContentEntry
	if err := json.Unmarshal(raw, &asList); err == nil && len(asList) > 0 {
		first := asList[0]
		if first.Type == "text" && !strings.HasPrefix(first.Text, "<") && len(first.Text) > 10 {
			return first.Text, true
		}
	}
	return "", false
}

// IterUserPrompts streams real free-text user prompts (for the
// fidelity layer to correlate against). Only main-session transcripts
// are considered — parser.py's iter_user_prompts globs *//*.jsonl only,
// never descending into subagents/, so this matches that scope exactly.
func IterUserPrompts(projectsDir string, yield func(models.UserPrompt) bool) error {
	files, err := listTranscripts(projectsDir)
	if err != nil {
		return err
	}
	for _, tf := range files {
		if tf.origin != models.OriginMain {
			continue
		}
		if !scanUserPrompts(tf, yield) {
			return nil
		}
	}
	return nil
}

func scanUserPrompts(tf transcriptFile, yield func(models.UserPrompt) bool) bool {
	f, err := os.Open(tf.path)
	if err != nil {
		return true
	}
	defer f.Close()

	scanner := bufio.NewScanner(f)
	scanner.Buffer(make([]byte, 0, 1024*1024), 64*1024*1024)
	for scanner.Scan() {
		data, ok := loadLine(scanner.Bytes())
		if !ok || data.Type != "user" {
			continue
		}
		// Synthetic/meta records are not real user asks: isMeta lines,
		// subagent sidechains (e.g. title-generator prompts), and
		// tool-result carriers all masquerade as type:"user".
		if data.IsMeta || data.IsSidechain || len(data.ToolUseResult) > 0 || data.SourceToolAssistantUUID != "" {
			continue
		}
		// promptSource "sdk"/"system" marks harness-generated prompts
		// (e.g. conversation-title generators); "typed"/"queued" are
		// real user input, "" (absent) predates the field — keep those.
		if data.PromptSource == "sdk" || data.PromptSource == "system" {
			continue
		}

		if data.SessionID == "" || data.Timestamp == "" {
			continue
		}
		ts, ok := parseTimestamp(data.Timestamp)
		if !ok {
			continue
		}
		if data.Message == nil {
			continue
		}

		text, ok := extractPromptText(data.Message.Content)
		if !ok {
			continue
		}
		if len(text) > 500 {
			text = text[:500]
		}

		projectPath := data.Cwd
		if projectPath == "" {
			projectPath = tf.fallbackProjectPath
		}

		prompt := models.UserPrompt{
			Text:        text,
			SessionID:   data.SessionID,
			ProjectPath: projectPath,
			Timestamp:   ts,
		}
		if !yield(prompt) {
			return false
		}
	}
	return true
}
