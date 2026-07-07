package parser

import (
	"sort"
	"testing"

	"github.com/plinde/skillscope/internal/models"
)

const testdataDir = "testdata/corpus"

func collectInvocations(t *testing.T) []models.SkillInvocation {
	t.Helper()
	var invs []models.SkillInvocation
	if err := IterInvocations(testdataDir, func(inv models.SkillInvocation) bool {
		invs = append(invs, inv)
		return true
	}); err != nil {
		t.Fatalf("IterInvocations: %v", err)
	}
	return invs
}

func collectPrompts(t *testing.T) []models.UserPrompt {
	t.Helper()
	var prompts []models.UserPrompt
	if err := IterUserPrompts(testdataDir, func(p models.UserPrompt) bool {
		prompts = append(prompts, p)
		return true
	}); err != nil {
		t.Fatalf("IterUserPrompts: %v", err)
	}
	return prompts
}

func findInvocation(invs []models.SkillInvocation, name string) (models.SkillInvocation, bool) {
	for _, inv := range invs {
		if inv.SkillName == name {
			return inv, true
		}
	}
	return models.SkillInvocation{}, false
}

// TestIterInvocations_RecordClassification covers user-slash extraction,
// claude-proactive extraction, EXCLUDED_COMMANDS skip, cwd fallback
// decoding, and malformed/incomplete-line skip, all against the fixture
// corpus in testdata/corpus.
func TestIterInvocations_RecordClassification(t *testing.T) {
	invs := collectInvocations(t)

	t.Run("user-slash extraction with args", func(t *testing.T) {
		inv, ok := findInvocation(invs, "mytool")
		if !ok {
			t.Fatal("expected a 'mytool' invocation")
		}
		if inv.TriggerType != models.TriggerUserSlash {
			t.Errorf("TriggerType = %q, want %q", inv.TriggerType, models.TriggerUserSlash)
		}
		if inv.Args != "foo bar" {
			t.Errorf("Args = %q, want %q", inv.Args, "foo bar")
		}
		if inv.ProjectPath != "/Users/test/project" {
			t.Errorf("ProjectPath = %q, want cwd-derived path", inv.ProjectPath)
		}
		if inv.Origin != models.OriginMain {
			t.Errorf("Origin = %q, want %q", inv.Origin, models.OriginMain)
		}
	})

	t.Run("claude-proactive extraction via skill field", func(t *testing.T) {
		var proactive []models.SkillInvocation
		for _, inv := range invs {
			if inv.SkillName == "mytool" && inv.TriggerType == models.TriggerClaudeProactive {
				proactive = append(proactive, inv)
			}
		}
		if len(proactive) != 1 {
			t.Fatalf("expected exactly 1 claude-proactive 'mytool' invocation, got %d", len(proactive))
		}
		if proactive[0].Args != "baz" {
			t.Errorf("Args = %q, want %q", proactive[0].Args, "baz")
		}
	})

	t.Run("claude-proactive falls back to command field when skill is empty", func(t *testing.T) {
		inv, ok := findInvocation(invs, "othertool")
		if !ok {
			t.Fatal("expected an 'othertool' invocation (command-field fallback)")
		}
		if inv.TriggerType != models.TriggerClaudeProactive {
			t.Errorf("TriggerType = %q, want %q", inv.TriggerType, models.TriggerClaudeProactive)
		}
	})

	t.Run("non-Skill tool_use is ignored", func(t *testing.T) {
		if _, ok := findInvocation(invs, "Bash"); ok {
			t.Error("Bash tool_use should not be classified as a skill invocation")
		}
	})

	t.Run("EXCLUDED_COMMANDS skipped case-insensitively", func(t *testing.T) {
		if _, ok := findInvocation(invs, "Help"); ok {
			t.Error("'/Help' should be skipped as an excluded command (case-insensitive)")
		}
		if _, ok := findInvocation(invs, "help"); ok {
			t.Error("'help' should be skipped as an excluded command")
		}
	})

	t.Run("cwd fallback to decoded project dir when line has no cwd", func(t *testing.T) {
		inv, ok := findInvocation(invs, "anothertool")
		if !ok {
			t.Fatal("expected an 'anothertool' invocation")
		}
		want := "/Users/test/project"
		if inv.ProjectPath != want {
			t.Errorf("ProjectPath = %q, want %q (decoded from project dir name)", inv.ProjectPath, want)
		}
	})

	t.Run("malformed line and missing sessionId are skipped", func(t *testing.T) {
		if _, ok := findInvocation(invs, "skiptool"); ok {
			t.Error("'skiptool' line has no sessionId and should be skipped")
		}
	})

	t.Run("cwd fallback across a whole file with no cwd field at all", func(t *testing.T) {
		inv, ok := findInvocation(invs, "fallbacktool")
		if !ok {
			t.Fatal("expected a 'fallbacktool' invocation")
		}
		want := "/Users/test/nocwd/app"
		if inv.ProjectPath != want {
			t.Errorf("ProjectPath = %q, want %q", inv.ProjectPath, want)
		}
	})
}

// TestIterInvocations_SubagentOrigin covers Feature 2: subagent
// transcripts (nested under a subagents/ dir at any depth) are
// classified with Origin == subagent, distinct from main-session files.
func TestIterInvocations_SubagentOrigin(t *testing.T) {
	invs := collectInvocations(t)

	inv, ok := findInvocation(invs, "subagenttool")
	if !ok {
		t.Fatal("expected a 'subagenttool' invocation from the subagents/ fixture")
	}
	if inv.Origin != models.OriginSubagent {
		t.Errorf("Origin = %q, want %q", inv.Origin, models.OriginSubagent)
	}

	inv2, ok := findInvocation(invs, "subagentskill")
	if !ok {
		t.Fatal("expected a 'subagentskill' invocation from the subagents/ fixture")
	}
	if inv2.Origin != models.OriginSubagent {
		t.Errorf("Origin = %q, want %q", inv2.Origin, models.OriginSubagent)
	}

	mainInv, ok := findInvocation(invs, "mytool")
	if !ok {
		t.Fatal("expected a main-session 'mytool' invocation")
	}
	if mainInv.Origin != models.OriginMain {
		t.Errorf("main-session invocation Origin = %q, want %q", mainInv.Origin, models.OriginMain)
	}
}

// TestIterUserPrompts_NoiseFilters covers each of the noise-line filters
// applied before a type:"user" line is treated as a real user prompt.
func TestIterUserPrompts_NoiseFilters(t *testing.T) {
	prompts := collectPrompts(t)

	texts := make([]string, 0, len(prompts))
	for _, p := range prompts {
		texts = append(texts, p.Text)
	}
	sort.Strings(texts)

	contains := func(needle string) bool {
		for _, s := range texts {
			if s == needle {
				return true
			}
		}
		return false
	}

	if !contains("This is a real user prompt about something important") {
		t.Error("expected the genuine free-text prompt to be included")
	}
	if !contains("This is a list-content real user prompt that should count") {
		t.Error("expected the list-content ([{type:text}]) prompt to be included")
	}

	excluded := []string{
		"This meta line should not count as a prompt",
		"This sidechain line should not count as a prompt",
		"This has a tool use result and should not count",
		"This has a source tool assistant uuid and should not count",
		"This is sdk generated and should not count as a real prompt",
		"This is system generated and should not count as a real prompt",
	}
	for _, e := range excluded {
		if contains(e) {
			t.Errorf("expected noise prompt to be filtered: %q", e)
		}
	}

	if contains("short") {
		t.Error("a prompt with len<=10 should be excluded by extractPromptText")
	}

	// Slash-command wrapper lines (type:"user" with XML content) must
	// never surface as prompts either, since extractPromptText excludes
	// any string content that starts with "<".
	for _, text := range texts {
		if len(text) > 0 && text[0] == '<' {
			t.Errorf("prompt text should never start with '<': %q", text)
		}
	}
}

// TestDecodeProjectDir covers the lossy dash-to-slash project directory
// decode used as a cwd fallback.
func TestDecodeProjectDir(t *testing.T) {
	cases := []struct {
		name string
		in   string
		want string
	}{
		{"typical encoded path", "-Users-test-project", "/Users/test/project"},
		{"already-prefixed slash is not doubled", "/already-slash", "/already/slash"},
		{"no leading dash still gets prefixed", "Users-test", "/Users/test"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := decodeProjectDir(tc.in)
			if got != tc.want {
				t.Errorf("decodeProjectDir(%q) = %q, want %q", tc.in, got, tc.want)
			}
		})
	}
}

// TestLoadLine covers loadLine's whitespace-trim, empty-line, and
// malformed-JSON skip behavior.
func TestLoadLine(t *testing.T) {
	cases := []struct {
		name    string
		in      string
		wantOK  bool
		wantSID string
	}{
		{"valid line", `{"sessionId":"abc","timestamp":"2026-01-01T00:00:00Z","type":"user"}`, true, "abc"},
		{"empty line", "", false, ""},
		{"whitespace-only line", "   \t  ", false, ""},
		{"malformed JSON", "not-json-at-all{{{", false, ""},
		{"line with surrounding whitespace", `  {"sessionId":"xyz","type":"user"}  `, true, "xyz"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			data, ok := loadLine([]byte(tc.in))
			if ok != tc.wantOK {
				t.Fatalf("loadLine(%q) ok = %v, want %v", tc.in, ok, tc.wantOK)
			}
			if ok && data.SessionID != tc.wantSID {
				t.Errorf("SessionID = %q, want %q", data.SessionID, tc.wantSID)
			}
		})
	}
}

// TestParseTimestamp covers both accepted timestamp layouts and the
// unparseable-input rejection.
func TestParseTimestamp(t *testing.T) {
	cases := []struct {
		name   string
		in     string
		wantOK bool
	}{
		{"RFC3339Nano", "2026-01-01T10:00:00.123456789Z", true},
		{"RFC3339", "2026-01-01T10:00:00Z", true},
		{"garbage", "not-a-timestamp", false},
		{"empty", "", false},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			_, ok := parseTimestamp(tc.in)
			if ok != tc.wantOK {
				t.Errorf("parseTimestamp(%q) ok = %v, want %v", tc.in, ok, tc.wantOK)
			}
		})
	}
}
