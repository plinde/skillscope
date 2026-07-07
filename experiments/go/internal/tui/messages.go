package tui

import "github.com/plinde/skillscope/internal/models"

// invocationsLoadedMsg carries the full (unfiltered-by-window)
// invocation set once the background load completes.
type invocationsLoadedMsg struct {
	invocations []models.SkillInvocation
	err         error
}
