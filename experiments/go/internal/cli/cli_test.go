package cli

import (
	"testing"
	"time"

	"github.com/plinde/skillscope/internal/models"
)

// TestParseSince_RelativeDurations covers the 7d/30d/12w/6m/1y relative
// duration forms, checking the result lands within the correct window
// relative to time.Now() (avoiding brittle exact-time comparisons).
func TestParseSince_RelativeDurations(t *testing.T) {
	cases := []struct {
		name       string
		in         string
		wantWithin time.Duration // acceptable slop around the expected offset
		wantOffset time.Duration // expected now - result
	}{
		{"7 days", "7d", time.Hour, 7 * 24 * time.Hour},
		{"30 days", "30d", time.Hour, 30 * 24 * time.Hour},
		{"12 weeks", "12w", time.Hour, 12 * 7 * 24 * time.Hour},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			got, err := ParseSince(tc.in)
			if err != nil {
				t.Fatalf("ParseSince(%q) error: %v", tc.in, err)
			}
			offset := time.Since(got)
			diff := offset - tc.wantOffset
			if diff < 0 {
				diff = -diff
			}
			if diff > tc.wantWithin {
				t.Errorf("ParseSince(%q) offset = %v, want ~%v (+/- %v)", tc.in, offset, tc.wantOffset, tc.wantWithin)
			}
		})
	}

	t.Run("months use AddDate month arithmetic", func(t *testing.T) {
		got, err := ParseSince("6m")
		if err != nil {
			t.Fatalf("ParseSince(\"6m\") error: %v", err)
		}
		want := time.Now().UTC().AddDate(0, -6, 0)
		if got.Sub(want) > time.Minute || want.Sub(got) > time.Minute {
			t.Errorf("ParseSince(\"6m\") = %v, want ~%v", got, want)
		}
	})

	t.Run("years use AddDate year arithmetic", func(t *testing.T) {
		got, err := ParseSince("1y")
		if err != nil {
			t.Fatalf("ParseSince(\"1y\") error: %v", err)
		}
		want := time.Now().UTC().AddDate(-1, 0, 0)
		if got.Sub(want) > time.Minute || want.Sub(got) > time.Minute {
			t.Errorf("ParseSince(\"1y\") = %v, want ~%v", got, want)
		}
	})
}

// TestParseSince_LiteralDate covers the YYYY-MM-DD literal-date form,
// preserved for compatibility with the Python reference's format.
func TestParseSince_LiteralDate(t *testing.T) {
	got, err := ParseSince("2026-03-15")
	if err != nil {
		t.Fatalf("ParseSince returned error: %v", err)
	}
	want := time.Date(2026, 3, 15, 0, 0, 0, 0, time.UTC)
	if !got.Equal(want) {
		t.Errorf("ParseSince(\"2026-03-15\") = %v, want %v", got, want)
	}
}

// TestParseSince_InvalidInput covers the error path for values that are
// neither a relative duration nor a literal YYYY-MM-DD date.
func TestParseSince_InvalidInput(t *testing.T) {
	cases := []string{"", "garbage", "7 days", "2026/03/15", "10x", "-5d"}
	for _, in := range cases {
		t.Run(in, func(t *testing.T) {
			_, err := ParseSince(in)
			if err == nil {
				t.Errorf("ParseSince(%q) expected an error, got nil", in)
			}
		})
	}
}

// TestMatchesOrigin covers the --origin filter's exact-match and
// permissive-default behavior.
func TestMatchesOrigin(t *testing.T) {
	mainInv := models.SkillInvocation{Origin: models.OriginMain}
	subInv := models.SkillInvocation{Origin: models.OriginSubagent}

	cases := []struct {
		name   string
		inv    models.SkillInvocation
		origin string
		want   bool
	}{
		{"empty origin matches main", mainInv, "", true},
		{"empty origin matches subagent", subInv, "", true},
		{"'all' matches main", mainInv, "all", true},
		{"'all' matches subagent", subInv, "all", true},
		{"'main' matches main invocation", mainInv, "main", true},
		{"'main' rejects subagent invocation", subInv, "main", false},
		{"'subagent' matches subagent invocation", subInv, "subagent", true},
		{"'subagent' rejects main invocation", mainInv, "subagent", false},
		{"unrecognized origin value is permissive", mainInv, "bogus", true},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := matchesOrigin(tc.inv, tc.origin); got != tc.want {
				t.Errorf("matchesOrigin(origin=%q, filter=%q) = %v, want %v", tc.inv.Origin, tc.origin, got, tc.want)
			}
		})
	}
}
