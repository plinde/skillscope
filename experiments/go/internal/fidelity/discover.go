// Package fidelity is the trigger-fidelity eval layer: skill discovery
// + offline heuristic classification. No LLM calls. Correlates real
// user prompts against skill frontmatter descriptions to flag two
// failure modes:
//
//   - under-triggering: a skill's description plausibly matches a
//     prompt, but the skill was never invoked in that session
//   - over-triggering: a skill was invoked in a session, but no prompt
//     in that session plausibly matches its description
//
// This port replaces fidelity.py's ">=2 raw keyword hits" rule (which
// flagged 346/357 skills as under-triggered on the live corpus) with
// TF-IDF weighting: see fidelity.go for the scoring model.
package fidelity

import (
	"os"
	"path/filepath"
	"sort"
	"strings"

	"gopkg.in/yaml.v3"

	"github.com/plinde/skillscope/internal/models"
)

// DefaultSkillsDirs mirrors fidelity.py's DEFAULT_SKILLS_DIRS.
func DefaultSkillsDirs(home string) []string {
	return []string{
		filepath.Join(home, ".agents", "skills"),
		filepath.Join(home, ".claude", "skills"),
	}
}

// PluginMarketplacesDir mirrors fidelity.py's PLUGIN_MARKETPLACES_DIR.
func PluginMarketplacesDir(home string) string {
	return filepath.Join(home, ".claude", "plugins", "marketplaces")
}

type frontmatter struct {
	Name        string `yaml:"name"`
	Description string `yaml:"description"`
}

func parseFrontmatter(content string) (frontmatter, bool) {
	if !strings.HasPrefix(content, "---") {
		return frontmatter{}, false
	}
	parts := strings.SplitN(content, "---", 3)
	if len(parts) < 3 {
		return frontmatter{}, false
	}
	var fm frontmatter
	if err := yaml.Unmarshal([]byte(parts[1]), &fm); err != nil {
		return frontmatter{}, false
	}
	return fm, true
}

type skillCandidate struct {
	path   string
	source string
}

// iterSkillMDCandidates yields (SKILL.md path, source) pairs, deduped
// by resolved path — matching fidelity.py's _iter_skill_md_candidates.
func iterSkillMDCandidates(skillsDirs []string, pluginMarketplacesDir string) []skillCandidate {
	seen := map[string]struct{}{}
	var candidates []skillCandidate

	addIfUnseen := func(path, source string) {
		resolved, err := filepath.EvalSymlinks(path)
		if err != nil {
			resolved = path
		}
		if _, ok := seen[resolved]; ok {
			return
		}
		seen[resolved] = struct{}{}
		candidates = append(candidates, skillCandidate{path: path, source: source})
	}

	for _, skillsDir := range skillsDirs {
		info, err := os.Stat(skillsDir)
		if err != nil || !info.IsDir() {
			continue
		}
		entries, err := os.ReadDir(skillsDir)
		if err != nil {
			continue
		}
		names := make([]string, 0, len(entries))
		for _, e := range entries {
			names = append(names, e.Name())
		}
		sort.Strings(names)
		for _, name := range names {
			skillMD := filepath.Join(skillsDir, name, "SKILL.md")
			if info, err := os.Stat(skillMD); err != nil || info.IsDir() {
				continue
			}
			addIfUnseen(skillMD, "user")
		}
	}

	if info, err := os.Stat(pluginMarketplacesDir); err == nil && info.IsDir() {
		matches, _ := filepath.Glob(filepath.Join(pluginMarketplacesDir, "*", "*", "skills", "*", "SKILL.md"))
		sort.Strings(matches)
		for _, m := range matches {
			addIfUnseen(m, "plugin")
		}
	}

	return candidates
}

// DiscoverSkills scans skill directories for SKILL.md files and parses
// their frontmatter. Files without valid YAML frontmatter are skipped
// gracefully.
func DiscoverSkills(skillsDirs []string, pluginMarketplacesDir string) []models.SkillDefinition {
	var results []models.SkillDefinition

	for _, cand := range iterSkillMDCandidates(skillsDirs, pluginMarketplacesDir) {
		content, err := os.ReadFile(cand.path)
		if err != nil {
			continue
		}
		fm, ok := parseFrontmatter(string(content))
		if !ok {
			continue
		}
		name := fm.Name
		if name == "" {
			name = filepath.Base(filepath.Dir(cand.path))
		}
		results = append(results, models.SkillDefinition{
			Name:        name,
			Description: fm.Description,
			Path:        filepath.Dir(cand.path),
			Source:      cand.source,
		})
	}

	return results
}
