package fidelity

import (
	"math"
	"os"
	"regexp"
	"sort"
	"strconv"
	"strings"

	"github.com/plinde/skillscope/internal/models"
	"github.com/plinde/skillscope/internal/parser"
)

var (
	wordRe           = regexp.MustCompile(`[a-z0-9][a-z0-9_-]*`)
	triggerSectionRe = regexp.MustCompile(`(?i)triggers?\s+on\s*:`)
	quotedPhraseRe   = regexp.MustCompile(`"([^"]{2,60})"`)
)

// stopwords: common English function words plus generic/domain-neutral
// words called out in the spec. Copied from fidelity.py's _STOPWORDS.
var stopwords = buildStopwords()

func buildStopwords() map[string]struct{} {
	words := []string{
		"a", "an", "the", "and", "or", "but", "if", "then", "else", "when",
		"while", "for", "to", "of", "in", "on", "at", "by", "with", "from",
		"into", "onto", "over", "under", "about", "as", "is", "are", "was",
		"were", "be", "been", "being", "this", "that", "these", "those",
		"it", "its", "you", "your", "yours", "i", "we", "our", "ours",
		"they", "their", "he", "she", "his", "her", "them", "not", "no",
		"do", "does", "did", "can", "could", "should", "would", "will",
		"shall", "may", "might", "must", "also", "than", "such", "any",
		"all", "some", "each", "every", "other", "another", "more", "most",
		"much", "many", "only", "just", "very", "so", "too", "own", "same",
		"here", "there", "what", "which", "who", "whom", "how", "why",
		"use", "uses", "using", "used", "skill", "skills", "tool", "tools",
		"user", "users", "claude", "agent", "agents", "code", "file",
		"files", "task", "tasks", "want", "wants", "need", "needs",
	}
	m := make(map[string]struct{}, len(words))
	for _, w := range words {
		m[w] = struct{}{}
	}
	return m
}

// Finding is one under- or over-triggered skill report entry.
type Finding struct {
	SkillName string
	Count     int
	Evidence  string
}

// Report holds the fidelity scan results.
type Report struct {
	UnderTriggered []Finding
	OverTriggered  []Finding
}

// skillKeywords is the TF-IDF-weighted keyword profile for one skill.
type skillKeywords struct {
	name    string             // lowercased
	weights map[string]float64 // token (len >= 4, not a stopword) -> IDF weight
	phrases []string           // lowercased multi-word phrases from "Triggers on:" sections
}

func extractTriggerPhrases(description string) []string {
	loc := triggerSectionRe.FindStringIndex(description)
	if loc == nil {
		return nil
	}
	tail := description[loc[1]:]
	seen := map[string]struct{}{}
	var phrases []string
	for _, m := range quotedPhraseRe.FindAllStringSubmatch(tail, -1) {
		p := strings.ToLower(strings.TrimSpace(m[1]))
		if p == "" {
			continue
		}
		if _, ok := seen[p]; !ok {
			seen[p] = struct{}{}
			phrases = append(phrases, p)
		}
	}
	return phrases
}

// skillTokens extracts the len>=4, non-stopword token set from a
// skill's "name description" text.
func skillTokens(skill models.SkillDefinition) map[string]struct{} {
	text := strings.ToLower(skill.Name + " " + skill.Description)
	tokens := wordRe.FindAllString(text, -1)
	set := make(map[string]struct{})
	for _, t := range tokens {
		if len(t) < 4 {
			continue
		}
		if _, stop := stopwords[t]; stop {
			continue
		}
		set[t] = struct{}{}
	}
	return set
}

// buildTFIDFKeywords computes IDF weights for every skill's keyword
// set. IDF is computed over the corpus of skill descriptions:
// idf(t) = ln(N / df(t)) + 1, where N is the number of skills and
// df(t) is the number of skill descriptions containing token t at
// least once. The "+1" floor keeps universally-shared tokens (df==N)
// from scoring zero — a skill still needs 2+ matched tokens under the
// default threshold, so this only affects borderline single-token
// cases.
func buildTFIDFKeywords(skills []models.SkillDefinition) map[string]skillKeywords {
	n := float64(len(skills))
	tokenSets := make([]map[string]struct{}, len(skills))
	df := map[string]int{}
	for i, s := range skills {
		tokenSets[i] = skillTokens(s)
		for t := range tokenSets[i] {
			df[t]++
		}
	}

	idf := map[string]float64{}
	for t, count := range df {
		idf[t] = math.Log(n/float64(count)) + 1
	}

	result := make(map[string]skillKeywords, len(skills))
	for i, s := range skills {
		weights := make(map[string]float64, len(tokenSets[i]))
		for t := range tokenSets[i] {
			weights[t] = idf[t]
		}
		result[s.Name] = skillKeywords{
			name:    strings.ToLower(s.Name),
			weights: weights,
			phrases: extractTriggerPhrases(s.Description),
		}
	}
	return result
}

func nameMatches(textLower, nameLower string) bool {
	if nameLower == "" {
		return false
	}
	normalize := func(s string) string {
		return regexp.MustCompile(`[-_]+`).ReplaceAllString(s, " ")
	}
	normalizedName := strings.TrimSpace(normalize(nameLower))
	if normalizedName == "" {
		return false
	}
	normalizedText := normalize(textLower)
	return strings.Contains(" "+normalizedText+" ", " "+normalizedName+" ")
}

// tfidfThreshold is the minimum summed IDF weight of matched tokens
// for a prompt to "plausibly match" a skill via keyword overlap alone
// (name match and quoted trigger phrases always short-circuit to a
// match regardless of this threshold). A single rare token (high IDF)
// can otherwise clear a weight-only threshold on its own — e.g. one
// distinctive but generic word landing in an unrelated prompt — so a
// minimum distinct-token-count floor (minMatchedTokens) is required in
// addition to the weight sum.
//
// Tuned empirically against the live ~/.claude/projects corpus (357
// discovered skills, via internal/fidelity/sweep_test.go). Weight-only
// scoring at any threshold left the under-triggered list flat around
// 340-357 (single-token matches dominating, no better than Python's
// raw ">=2 keyword hits" rule which flagged 346/357). Requiring >=6
// distinct matched tokens plus a weight sum of >=25.0 brings the
// keyword-overlap contribution down to near zero, landing the
// under-triggered list at its practical floor of ~53 skills on this
// corpus.
//
// That floor is *not* further reducible by tuning these two
// constants alone: it is dominated by the spec-mandated,
// verbatim-preserved instant name-match rule (nameMatches below)
// colliding with several skill names that are also common English
// words in this corpus's domain — "security", "research", "terraform",
// "okta", "teleport", "gh", "report", "todo", "vpn", "gcp", "mfa" —
// each of which legitimately appears in a large fraction of this PSEC
// engineer's prompts regardless of skill relevance. In isolation
// (keyword scoring fully disabled) the name-match rule alone produces
// 51 under-triggered skills at the old fixed count>=3 cutoff. This is
// a real corpus characteristic of a single, topically-concentrated
// user, not a tuning defect.
//
// Lever that actually closes the gap: minSessionCount (below). A
// common-word skill name (e.g. "security") legitimately matches a
// prompt-only-once-or-twice pattern across many sessions, but only
// fires as a *sustained* signal — many distinct matching sessions —
// for skills the engineer genuinely reaches for repeatedly; one-off
// common-word collisions rarely clear a higher session-count bar.
// Raising the report's minimum-session-count cutoff from a fixed 3 to
// a tunable, env-overridable default of 8 (mirroring the sibling Rust
// implementation's fix for the identical structural floor, including
// its env var names for cross-implementation consistency) brings the
// under-triggered list from ~53 down to 26 on this corpus — within the
// spec's 10-30 target band — without weakening the name-match rule
// itself. Override via SKILLSCOPE_MIN_SESSION_COUNT /
// SKILLSCOPE_TFIDF_THRESHOLD if a different corpus lands outside that
// band.
var (
	tfidfThreshold   = 25.0
	minMatchedTokens = 6

	// minSessionCount is the minimum number of distinct matching
	// sessions a skill needs to appear in the under-triggered report.
	// Resolved from SKILLSCOPE_MIN_SESSION_COUNT at the top of Run
	// (not here, at package-init time) so it composes with
	// t.Setenv-based testing.
	minSessionCount = defaultMinSessionCount
)

const (
	defaultMinSessionCount = 8

	// envMinSessionCount and envTFIDFThreshold match the names used by
	// the sibling Rust implementation (poc/rust-ratatui), so a user
	// tuning one implementation's fidelity thresholds gets the same
	// effect setting the same env var against either binary.
	envMinSessionCount = "SKILLSCOPE_MIN_SESSION_COUNT"
	envTFIDFThreshold  = "SKILLSCOPE_TFIDF_THRESHOLD"
)

// envInt reads an environment variable as an int, falling back to def
// if the variable is unset or unparseable.
func envInt(name string, def int) int {
	raw := os.Getenv(name)
	if raw == "" {
		return def
	}
	v, err := strconv.Atoi(raw)
	if err != nil {
		return def
	}
	return v
}

// envFloat reads an environment variable as a float64, falling back to
// def if the variable is unset or unparseable.
func envFloat(name string, def float64) float64 {
	raw := os.Getenv(name)
	if raw == "" {
		return def
	}
	v, err := strconv.ParseFloat(raw, 64)
	if err != nil {
		return def
	}
	return v
}

// promptMatchesSkill reports whether a prompt "plausibly matches" a
// skill: contains the skill name as a word, OR contains a quoted
// trigger phrase from a "Triggers on:" section, OR the prompt overlaps
// at least minMatchedTokens distinct keywords from the skill's profile
// with a summed IDF weight meeting tfidfThreshold.
func promptMatchesSkill(textLower string, tokens map[string]struct{}, kw skillKeywords) bool {
	if nameMatches(textLower, kw.name) {
		return true
	}
	for _, phrase := range kw.phrases {
		if strings.Contains(textLower, phrase) {
			return true
		}
	}
	var score float64
	var matched int
	for t := range tokens {
		if w, ok := kw.weights[t]; ok {
			score += w
			matched++
		}
	}
	return matched >= minMatchedTokens && score >= tfidfThreshold
}

// isNoisePrompt applies Feature 1's additional prompt filters, beyond
// the core parser's transcript-level filters: plan-mode injections,
// teammate-message wrapper text, and prompts too short to carry real
// intent.
func isNoisePrompt(text string) bool {
	if len(text) < 15 {
		return true
	}
	if strings.HasPrefix(text, "Implement the following plan:") {
		return true
	}
	if strings.Contains(text, "<teammate-message") {
		return true
	}
	return false
}

// Options configures RunFidelity.
type Options struct {
	ProjectsDir           string
	SkillsDirs            []string
	PluginMarketplacesDir string
	Since                 *timeFilter
}

// timeFilter is a minimal since-cutoff; kept as its own type so callers
// don't need to import time in the common case of an unfiltered run.
type timeFilter struct {
	unixNano int64
}

// NewSinceFilter builds a since-cutoff from a Unix-nanosecond timestamp.
func NewSinceFilter(unixNano int64) *timeFilter { return &timeFilter{unixNano: unixNano} }

// Run executes the fidelity scan: discover skills, stream invocations
// and prompts, correlate per-session, and bucket into under/over
// triggered findings.
func Run(opts Options) (Report, error) {
	// Resolved at call time (not package init) so tests can use
	// t.Setenv to exercise both the override and fallback paths.
	tfidfThreshold = envFloat(envTFIDFThreshold, 25.0)
	minSessionCount = envInt(envMinSessionCount, defaultMinSessionCount)

	skills := DiscoverSkills(opts.SkillsDirs, opts.PluginMarketplacesDir)
	skillKW := buildTFIDFKeywords(skills)

	sessionInvocations := map[string]map[string]struct{}{}
	err := parser.IterInvocations(opts.ProjectsDir, func(inv models.SkillInvocation) bool {
		if opts.Since != nil && inv.Timestamp.UnixNano() < opts.Since.unixNano {
			return true
		}
		set, ok := sessionInvocations[inv.SessionID]
		if !ok {
			set = map[string]struct{}{}
			sessionInvocations[inv.SessionID] = set
		}
		set[inv.SkillName] = struct{}{}
		return true
	})
	if err != nil {
		return Report{}, err
	}

	sessionPrompts := map[string][]models.UserPrompt{}
	err = parser.IterUserPrompts(opts.ProjectsDir, func(p models.UserPrompt) bool {
		if opts.Since != nil && p.Timestamp.UnixNano() < opts.Since.unixNano {
			return true
		}
		if isNoisePrompt(p.Text) {
			return true
		}
		sessionPrompts[p.SessionID] = append(sessionPrompts[p.SessionID], p)
		return true
	})
	if err != nil {
		return Report{}, err
	}

	underCounts := map[string]int{}
	underExamples := map[string][]string{}
	overCounts := map[string]int{}
	overExamples := map[string][]string{}

	allSessions := map[string]struct{}{}
	for sid := range sessionInvocations {
		allSessions[sid] = struct{}{}
	}
	for sid := range sessionPrompts {
		allSessions[sid] = struct{}{}
	}

	for sessionID := range allSessions {
		prompts := sessionPrompts[sessionID]
		invoked := sessionInvocations[sessionID]

		sessionMatched := map[string]struct{}{}
		matchedSnippet := map[string]string{}
		for _, prompt := range prompts {
			textLower := strings.ToLower(prompt.Text)
			tokenList := wordRe.FindAllString(textLower, -1)
			tokens := make(map[string]struct{}, len(tokenList))
			for _, t := range tokenList {
				tokens[t] = struct{}{}
			}
			for skillName, kw := range skillKW {
				if _, already := sessionMatched[skillName]; already {
					continue
				}
				if promptMatchesSkill(textLower, tokens, kw) {
					sessionMatched[skillName] = struct{}{}
					snippet := prompt.Text
					if len(snippet) > 120 {
						snippet = snippet[:120]
					}
					matchedSnippet[skillName] = snippet
				}
			}
		}

		for skillName := range sessionMatched {
			if _, ok := invoked[skillName]; ok {
				continue
			}
			underCounts[skillName]++
			if len(underExamples[skillName]) < 3 {
				underExamples[skillName] = append(underExamples[skillName], matchedSnippet[skillName])
			}
		}

		for skillName := range invoked {
			if _, ok := sessionMatched[skillName]; ok {
				continue
			}
			overCounts[skillName]++
			snippet := "(no user prompt text in session)"
			if len(prompts) > 0 {
				snippet = prompts[0].Text
				if len(snippet) > 120 {
					snippet = snippet[:120]
				}
			}
			if len(overExamples[skillName]) < 3 {
				overExamples[skillName] = append(overExamples[skillName], snippet)
			}
		}
	}

	var under []Finding
	for name, count := range underCounts {
		if count < minSessionCount {
			continue
		}
		under = append(under, Finding{SkillName: name, Count: count, Evidence: strings.Join(underExamples[name], " | ")})
	}
	sort.SliceStable(under, func(i, j int) bool { return under[i].Count > under[j].Count })

	var over []Finding
	for name, count := range overCounts {
		over = append(over, Finding{SkillName: name, Count: count, Evidence: strings.Join(overExamples[name], " | ")})
	}
	sort.SliceStable(over, func(i, j int) bool { return over[i].Count > over[j].Count })

	return Report{UnderTriggered: under, OverTriggered: over}, nil
}
