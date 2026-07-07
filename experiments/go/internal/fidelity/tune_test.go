package fidelity

import (
	"fmt"
	"os"
	"testing"
)

// TestTuneThreshold is a manual empirical-tuning harness, not a real
// assertion test. Run with: go test ./internal/fidelity/ -run TestTuneThreshold -v
func TestTuneThreshold(t *testing.T) {
	if os.Getenv("SKILLSCOPE_TUNE") == "" {
		t.Skip("set SKILLSCOPE_TUNE=1 to run against the live corpus")
	}
	home, _ := os.UserHomeDir()
	opts := Options{
		ProjectsDir:           home + "/.claude/projects",
		SkillsDirs:            DefaultSkillsDirs(home),
		PluginMarketplacesDir: PluginMarketplacesDir(home),
	}
	report, err := Run(opts)
	if err != nil {
		t.Fatal(err)
	}
	fmt.Println("under-triggered:", len(report.UnderTriggered))
	fmt.Println("over-triggered:", len(report.OverTriggered))
	for _, f := range report.UnderTriggered {
		fmt.Printf("  UNDER %-40s count=%d evidence=%q\n", f.SkillName, f.Count, f.Evidence)
	}
}
