package sessions

import (
	"os"
	"path/filepath"
	"testing"
)

const testdataDir = "testdata/corpus"

// TestLoad_MalformedIndexSkippedGracefully covers Load's best-effort
// behavior: a malformed sessions-index.json must not abort the load or
// surface an error — entries from other, valid index files still load.
// The malformed fixture is written to a temp dir at runtime (rather
// than committed under testdata/) so it doesn't trip repo-wide JSON
// validation hooks on a file that is deliberately invalid JSON.
func TestLoad_MalformedIndexSkippedGracefully(t *testing.T) {
	dir := t.TempDir()

	validDir := filepath.Join(dir, "proj1")
	if err := os.MkdirAll(validDir, 0o755); err != nil {
		t.Fatal(err)
	}
	validSrc, err := os.ReadFile(filepath.Join(testdataDir, "proj1", "sessions-index.json"))
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(validDir, "sessions-index.json"), validSrc, 0o644); err != nil {
		t.Fatal(err)
	}

	malformedDir := filepath.Join(dir, "proj2")
	if err := os.MkdirAll(malformedDir, 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(malformedDir, "sessions-index.json"), []byte("not valid json at all {{{\n"), 0o644); err != nil {
		t.Fatal(err)
	}

	idx, err := Load(dir)
	if err != nil {
		t.Fatalf("Load returned error: %v", err)
	}
	if _, ok := idx.Lookup("sess-with-summary"); !ok {
		t.Error("expected entries from the valid proj1 index to still be loaded despite proj2's malformed index")
	}
}

// TestLoad_SkipsEmptySessionID covers the empty-sessionId skip in Load.
func TestLoad_SkipsEmptySessionID(t *testing.T) {
	idx, err := Load(testdataDir)
	if err != nil {
		t.Fatalf("Load returned error: %v", err)
	}
	if _, ok := idx.Lookup(""); ok {
		t.Error("an entry with an empty sessionId must not be indexed")
	}
}

// TestDisplayTitle_FallbackChain covers the summary -> firstPrompt
// (truncated to 80 chars) -> raw UUID fallback chain.
func TestDisplayTitle_FallbackChain(t *testing.T) {
	idx, err := Load(testdataDir)
	if err != nil {
		t.Fatalf("Load returned error: %v", err)
	}

	t.Run("summary present takes priority", func(t *testing.T) {
		got := idx.DisplayTitle("sess-with-summary")
		want := "A concise session summary"
		if got != want {
			t.Errorf("DisplayTitle = %q, want %q", got, want)
		}
	})

	t.Run("falls back to truncated firstPrompt when summary is empty", func(t *testing.T) {
		got := idx.DisplayTitle("sess-firstprompt-only")
		if len(got) == 0 {
			t.Fatal("expected non-empty title")
		}
		runes := []rune(got)
		// 80 chars + the appended ellipsis rune.
		if len(runes) != 81 {
			t.Errorf("DisplayTitle length = %d, want 81 (80 truncated chars + ellipsis)", len(runes))
		}
		if runes[len(runes)-1] != '…' {
			t.Errorf("expected truncated title to end with an ellipsis, got %q", got)
		}
	})

	t.Run("falls back to raw sessionID when both summary and firstPrompt are empty", func(t *testing.T) {
		got := idx.DisplayTitle("sess-neither")
		if got != "sess-neither" {
			t.Errorf("DisplayTitle = %q, want raw sessionID %q", got, "sess-neither")
		}
	})

	t.Run("falls back to raw sessionID when there is no index entry at all", func(t *testing.T) {
		got := idx.DisplayTitle("sess-not-in-any-index")
		if got != "sess-not-in-any-index" {
			t.Errorf("DisplayTitle = %q, want raw sessionID", got)
		}
	})
}

// TestGitBranch covers GitBranch's indexed-value and no-entry cases.
func TestGitBranch(t *testing.T) {
	idx, err := Load(testdataDir)
	if err != nil {
		t.Fatalf("Load returned error: %v", err)
	}
	if got := idx.GitBranch("sess-with-summary"); got != "main" {
		t.Errorf("GitBranch = %q, want %q", got, "main")
	}
	if got := idx.GitBranch("sess-neither"); got != "" {
		t.Errorf("GitBranch for entry with no branch = %q, want empty string", got)
	}
	if got := idx.GitBranch("no-such-session"); got != "" {
		t.Errorf("GitBranch for unknown session = %q, want empty string", got)
	}
}

// TestNilIndexIsSafe covers the nil-receiver safety documented on Lookup.
func TestNilIndexIsSafe(t *testing.T) {
	var idx *Index
	if _, ok := idx.Lookup("anything"); ok {
		t.Error("Lookup on a nil *Index must return ok=false")
	}
	if got := idx.DisplayTitle("raw-id"); got != "raw-id" {
		t.Errorf("DisplayTitle on a nil *Index = %q, want the raw id echoed back", got)
	}
	if got := idx.GitBranch("anything"); got != "" {
		t.Errorf("GitBranch on a nil *Index = %q, want empty string", got)
	}
}

// TestLoad_NoIndexFilesAtAll covers Load against a directory with no
// sessions-index.json files anywhere (e.g. an unindexed corpus).
func TestLoad_NoIndexFilesAtAll(t *testing.T) {
	idx, err := Load("testdata/empty-corpus")
	if err != nil {
		t.Fatalf("Load returned error for a directory with no index files: %v", err)
	}
	if _, ok := idx.Lookup("anything"); ok {
		t.Error("expected no entries from an empty corpus")
	}
}
