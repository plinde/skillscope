package fidelity

import (
	"strings"
	"testing"

	"github.com/plinde/skillscope/internal/models"
)

const runCorpusTestdata = "testdata/run-corpus"

// TestNameMatches covers the verbatim-name instant-match rule, including
// its dash/underscore normalization and word-boundary requirement.
func TestNameMatches(t *testing.T) {
	cases := []struct {
		name      string
		textLower string
		nameLower string
		want      bool
	}{
		{"exact word match", "please run the terraform plan now", "terraform", true},
		{"dash-normalized skill name matches spaced text", "run the vault helper please", "vault-helper", true},
		{"underscore-normalized skill name matches spaced text", "run the vault helper please", "vault_helper", true},
		{"dashed text matches spaced name", "run the vault-helper please", "vault helper", true},
		{"no match when word absent", "please run the ansible playbook", "terraform", false},
		{"substring inside another word does not match", "aterraformx is not a real word", "terraform", false},
		{"empty name never matches", "anything at all", "", false},
		{"match at start of text", "terraform apply now", "terraform", true},
		{"match at end of text", "please run terraform", "terraform", true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := nameMatches(tc.textLower, tc.nameLower); got != tc.want {
				t.Errorf("nameMatches(%q, %q) = %v, want %v", tc.textLower, tc.nameLower, got, tc.want)
			}
		})
	}
}

// TestExtractTriggerPhrases covers the "Triggers on:" quoted-phrase
// extraction, including dedup and the no-section case.
func TestExtractTriggerPhrases(t *testing.T) {
	cases := []struct {
		name        string
		description string
		want        []string
	}{
		{
			"single triggers-on section with two phrases",
			`Does a thing. Triggers on: "deploy the app" and "ship it now".`,
			[]string{"deploy the app", "ship it now"},
		},
		{
			"case-insensitive and flexible whitespace in the header",
			`Some skill. TRIGGER  ON:   "quick phrase".`,
			[]string{"quick phrase"},
		},
		{
			"no triggers-on section yields nil",
			`Just a plain description with no trigger phrases at all.`,
			nil,
		},
		{
			"duplicate phrases are deduped",
			`Triggers on: "same phrase" or "same phrase" or "SAME PHRASE".`,
			[]string{"same phrase"},
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got := extractTriggerPhrases(tc.description)
			if len(got) != len(tc.want) {
				t.Fatalf("extractTriggerPhrases(%q) = %v, want %v", tc.description, got, tc.want)
			}
			for i := range got {
				if got[i] != tc.want[i] {
					t.Errorf("phrase[%d] = %q, want %q", i, got[i], tc.want[i])
				}
			}
		})
	}
}

// TestPromptMatchesSkill_InstantMatchRules covers the two rules that
// short-circuit to a match regardless of the TF-IDF threshold: verbatim
// name match and quoted trigger-phrase substring match.
func TestPromptMatchesSkill_InstantMatchRules(t *testing.T) {
	kw := skillKeywords{
		name:    "myskill",
		weights: map[string]float64{}, // no keyword weights at all
		phrases: []string{"do the special thing"},
	}

	t.Run("name match short-circuits with zero keyword overlap", func(t *testing.T) {
		text := "please use myskill for this"
		tokens := map[string]struct{}{} // deliberately empty
		if !promptMatchesSkill(text, tokens, kw) {
			t.Error("expected name match to short-circuit to true")
		}
	})

	t.Run("quoted trigger phrase substring short-circuits", func(t *testing.T) {
		text := "i want you to do the special thing right now"
		tokens := map[string]struct{}{}
		if !promptMatchesSkill(text, tokens, kw) {
			t.Error("expected trigger-phrase substring match to short-circuit to true")
		}
	})

	t.Run("no name, no phrase, no keyword overlap does not match", func(t *testing.T) {
		text := "completely unrelated text with no overlap"
		tokens := map[string]struct{}{}
		if promptMatchesSkill(text, tokens, kw) {
			t.Error("expected no match")
		}
	})
}

// TestPromptMatchesSkill_ThresholdBehavior covers the minMatchedTokens +
// tfidfThreshold combination governing keyword-overlap-only matches
// (i.e. when neither instant-match rule fires).
func TestPromptMatchesSkill_ThresholdBehavior(t *testing.T) {
	// Save and restore the tuned package-level constants so this test
	// is independent of whatever values production tuning currently
	// uses, and other tests aren't affected by this test's overrides.
	origThreshold, origMinTokens := tfidfThreshold, minMatchedTokens
	t.Cleanup(func() { tfidfThreshold, minMatchedTokens = origThreshold, origMinTokens })
	tfidfThreshold = 10.0
	minMatchedTokens = 3

	kw := skillKeywords{
		name: "zzznomatch", // deliberately won't appear in any test text
		weights: map[string]float64{
			"alpha":   4.0,
			"bravo":   4.0,
			"charlie": 4.0,
			"delta":   1.0,
		},
	}

	cases := []struct {
		name   string
		tokens map[string]struct{}
		want   bool
	}{
		{
			"meets both token-count floor and weight threshold",
			map[string]struct{}{"alpha": {}, "bravo": {}, "charlie": {}},
			true, // 3 tokens, weight 12.0 >= 10.0
		},
		{
			"meets weight threshold but not token-count floor",
			map[string]struct{}{"alpha": {}, "bravo": {}},
			false, // 2 tokens < minMatchedTokens=3, even though weight 8.0 is close
		},
		{
			"meets token-count floor but not weight threshold",
			map[string]struct{}{"delta": {}, "alpha": {}}, // only 2 known tokens anyway
			false,
		},
		{
			"no overlapping tokens at all",
			map[string]struct{}{"unrelated": {}, "words": {}, "here": {}},
			false,
		},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			text := "no name or phrase match in this text"
			if got := promptMatchesSkill(text, tc.tokens, kw); got != tc.want {
				t.Errorf("promptMatchesSkill() = %v, want %v", got, tc.want)
			}
		})
	}
}

// TestBuildTFIDFKeywords covers the IDF weighting: a token appearing in
// every skill's description should score lower than a token appearing
// in only one.
func TestBuildTFIDFKeywords(t *testing.T) {
	skills := []models.SkillDefinition{
		{Name: "skillone", Description: "handles common things and unique alpha"},
		{Name: "skilltwo", Description: "handles common things and unique bravo"},
		{Name: "skillthree", Description: "handles common things and unique charlie"},
	}
	kw := buildTFIDFKeywords(skills)

	one := kw["skillone"]
	commonWeight, ok := one.weights["common"]
	if !ok {
		t.Fatal("expected 'common' to be a keyword (len>=4, not a stopword)")
	}
	uniqueWeight, ok := one.weights["alpha"]
	if !ok {
		t.Fatal("expected 'alpha' to be a keyword")
	}
	if uniqueWeight <= commonWeight {
		t.Errorf("expected a token unique to one skill (weight %v) to score higher than a token shared by all skills (weight %v)", uniqueWeight, commonWeight)
	}
}

// TestSkillTokens covers the length and stopword filtering applied
// before a skill's description contributes to its keyword profile.
func TestSkillTokens(t *testing.T) {
	skill := models.SkillDefinition{
		Name:        "example",
		Description: "The tool can use this to help with terraform and aws deployment.",
	}
	tokens := skillTokens(skill)

	mustContain := []string{"terraform", "deployment"}
	for _, tok := range mustContain {
		if _, ok := tokens[tok]; !ok {
			t.Errorf("expected token %q to survive filtering", tok)
		}
	}

	mustNotContain := []string{"the", "can", "use", "this", "to", "aws", "tool", "and", "with"}
	for _, tok := range mustNotContain {
		if _, ok := tokens[tok]; ok {
			t.Errorf("expected token %q to be filtered (stopword or len<4)", tok)
		}
	}
}

// TestIsNoisePrompt covers Feature 1's additional prompt filters beyond
// the core parser's transcript-level filters.
func TestIsNoisePrompt(t *testing.T) {
	cases := []struct {
		name string
		text string
		want bool
	}{
		{"too short", "short text", true},
		{"plan-mode injection prefix", "Implement the following plan: do the thing", true},
		{"teammate message wrapper", "some prefix <teammate-message from=x> hello </teammate-message>", true},
		{"genuine long prompt", "This is a perfectly normal, sufficiently long user prompt with real intent.", false},
		{"exactly at the boundary length is not noise", strings.Repeat("x", 15), false},
		{"one under the boundary length is noise", strings.Repeat("x", 14), true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := isNoisePrompt(tc.text); got != tc.want {
				t.Errorf("isNoisePrompt(%q) = %v, want %v", tc.text, got, tc.want)
			}
		})
	}
}

// TestEnvInt covers the SKILLSCOPE_MIN_SESSION_COUNT override helper:
// unset, valid, and unparseable env values.
func TestEnvInt(t *testing.T) {
	const name = "SKILLSCOPE_TEST_ENV_INT"
	cases := []struct {
		name    string
		envVal  string
		setEnv  bool
		def     int
		wantVal int
	}{
		{"unset falls back to default", "", false, 8, 8},
		{"valid override", "12", true, 8, 12},
		{"unparseable falls back to default", "not-a-number", true, 8, 8},
		{"empty string falls back to default", "", true, 8, 8},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if tc.setEnv {
				t.Setenv(name, tc.envVal)
			}
			if got := envInt(name, tc.def); got != tc.wantVal {
				t.Errorf("envInt(%q, %d) = %d, want %d", name, tc.def, got, tc.wantVal)
			}
		})
	}
}

// TestEnvFloat covers the SKILLSCOPE_TFIDF_THRESHOLD override helper:
// unset, valid, and unparseable env values.
func TestEnvFloat(t *testing.T) {
	const name = "SKILLSCOPE_TEST_ENV_FLOAT"
	cases := []struct {
		name    string
		envVal  string
		setEnv  bool
		def     float64
		wantVal float64
	}{
		{"unset falls back to default", "", false, 25.0, 25.0},
		{"valid override", "10.5", true, 25.0, 10.5},
		{"unparseable falls back to default", "not-a-float", true, 25.0, 25.0},
		{"empty string falls back to default", "", true, 25.0, 25.0},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if tc.setEnv {
				t.Setenv(name, tc.envVal)
			}
			if got := envFloat(name, tc.def); got != tc.wantVal {
				t.Errorf("envFloat(%q, %g) = %g, want %g", name, tc.def, got, tc.wantVal)
			}
		})
	}
}

// TestRun_MinSessionCountCutoff exercises the minSessionCount lever
// end-to-end through Run: a fixture corpus has one skill
// ("widgettool") whose name is matched by a prompt in 5 distinct
// sessions, but the skill is never actually invoked in any of them —
// a pure under-triggered signal. With the default cutoff (8) it must
// NOT appear in the report (5 < 8); lowering the cutoff via
// SKILLSCOPE_MIN_SESSION_COUNT to something <= 5 must surface it.
func TestRun_MinSessionCountCutoff(t *testing.T) {
	opts := Options{
		ProjectsDir:           runCorpusTestdata + "/projects",
		SkillsDirs:            []string{runCorpusTestdata + "/skills"},
		PluginMarketplacesDir: runCorpusTestdata + "/nonexistent-plugins",
	}

	t.Run("default cutoff excludes a 5-session match", func(t *testing.T) {
		report, err := Run(opts)
		if err != nil {
			t.Fatal(err)
		}
		if f := findFinding(report.UnderTriggered, "widgettool"); f != nil {
			t.Errorf("widgettool unexpectedly under-triggered at default cutoff: %+v", f)
		}
	})

	t.Run("lowered cutoff via env surfaces the 5-session match", func(t *testing.T) {
		t.Setenv(envMinSessionCount, "5")
		report, err := Run(opts)
		if err != nil {
			t.Fatal(err)
		}
		f := findFinding(report.UnderTriggered, "widgettool")
		if f == nil {
			t.Fatal("expected widgettool to be under-triggered once cutoff is lowered to 5")
		}
		if f.Count != 5 {
			t.Errorf("widgettool count = %d, want 5", f.Count)
		}
	})

	t.Run("cutoff above 5 still excludes it", func(t *testing.T) {
		t.Setenv(envMinSessionCount, "6")
		report, err := Run(opts)
		if err != nil {
			t.Fatal(err)
		}
		if f := findFinding(report.UnderTriggered, "widgettool"); f != nil {
			t.Errorf("widgettool unexpectedly under-triggered at cutoff=6: %+v", f)
		}
	})
}

func findFinding(findings []Finding, name string) *Finding {
	for i := range findings {
		if findings[i].SkillName == name {
			return &findings[i]
		}
	}
	return nil
}
