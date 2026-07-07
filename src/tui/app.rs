//! TUI application state: three-level navigation (skills -> sessions -> invocations).

use crate::aggregate::{self, SessionEntry, SkillCountEntry};
use crate::models::SkillInvocation;
use crate::sessions::{SessionIndex, session_branch, session_label};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Skills,
    Sessions,
    Invocations,
    /// Session-scoped: per-skill counts within one session.
    SessionSkills,
    /// Session-scoped: flat chronological timeline across all skills.
    SessionTimeline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Total,
    Slash,
    Proactive,
    Subagent,
    LastSeen,
}

impl SortKey {
    fn next(self) -> Self {
        match self {
            SortKey::Total => SortKey::Slash,
            SortKey::Slash => SortKey::Proactive,
            SortKey::Proactive => SortKey::Subagent,
            SortKey::Subagent => SortKey::LastSeen,
            SortKey::LastSeen => SortKey::Total,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Total => "total",
            SortKey::Slash => "slash",
            SortKey::Proactive => "proactive",
            SortKey::Subagent => "subagent",
            SortKey::LastSeen => "last-seen",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeWindow {
    SevenDays,
    ThirtyDays,
    All,
}

impl TimeWindow {
    fn next(self) -> Self {
        match self {
            TimeWindow::SevenDays => TimeWindow::ThirtyDays,
            TimeWindow::ThirtyDays => TimeWindow::All,
            TimeWindow::All => TimeWindow::SevenDays,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TimeWindow::SevenDays => "7d",
            TimeWindow::ThirtyDays => "30d",
            TimeWindow::All => "all",
        }
    }

    fn cutoff(self) -> Option<DateTime<Utc>> {
        match self {
            TimeWindow::SevenDays => Some(Utc::now() - chrono::Duration::days(7)),
            TimeWindow::ThirtyDays => Some(Utc::now() - chrono::Duration::days(30)),
            TimeWindow::All => None,
        }
    }
}

pub struct App {
    all_invocations: Vec<SkillInvocation>,
    session_index: SessionIndex,

    pub level: Level,
    pub sort_key: SortKey,
    pub time_window: TimeWindow,
    pub filter: String,
    pub filter_editing: bool,
    pub selected: usize,

    pub selected_skill: Option<String>,
    pub selected_session: Option<String>,

    /// When set, the app is scoped to this session id: the invocation set
    /// was parsed from that session's transcripts only, the top level is
    /// SessionSkills/SessionTimeline, and go_back never reaches the global
    /// levels.
    pub session_scope: Option<String>,
}

pub struct SkillRow {
    pub name: String,
    pub stats: SkillCountEntry,
}

impl App {
    pub fn new(all_invocations: Vec<SkillInvocation>, session_index: SessionIndex) -> Self {
        Self {
            all_invocations,
            session_index,
            level: Level::Skills,
            sort_key: SortKey::Total,
            time_window: TimeWindow::All,
            filter: String::new(),
            filter_editing: false,
            selected: 0,
            selected_skill: None,
            selected_session: None,
            session_scope: None,
        }
    }

    /// Session-scoped app: `all_invocations` must already be limited to the
    /// scoped session's transcripts (see
    /// `parser::iter_invocations_for_session`).
    pub fn new_scoped(
        all_invocations: Vec<SkillInvocation>,
        session_index: SessionIndex,
        session_id: String,
    ) -> Self {
        let mut app = Self::new(all_invocations, session_index);
        app.level = Level::SessionSkills;
        app.selected_session = Some(session_id.clone());
        app.session_scope = Some(session_id);
        app
    }

    pub fn is_scoped_top_level(&self) -> bool {
        matches!(self.level, Level::SessionSkills | Level::SessionTimeline)
    }

    fn windowed_invocations(&self) -> Vec<&SkillInvocation> {
        match self.time_window.cutoff() {
            Some(cutoff) => self
                .all_invocations
                .iter()
                .filter(|i| i.timestamp >= cutoff)
                .collect(),
            None => self.all_invocations.iter().collect(),
        }
    }

    pub fn skill_rows(&self) -> Vec<SkillRow> {
        let owned: Vec<SkillInvocation> =
            self.windowed_invocations().into_iter().cloned().collect();
        let counts = aggregate::skill_counts(&owned);
        let mut rows: Vec<SkillRow> = counts
            .into_iter()
            .filter(|(name, _)| {
                self.filter.is_empty() || name.to_lowercase().contains(&self.filter.to_lowercase())
            })
            .map(|(name, stats)| SkillRow { name, stats })
            .collect();

        rows.sort_by(|a, b| match self.sort_key {
            SortKey::Total => b.stats.total.cmp(&a.stats.total),
            SortKey::Slash => b.stats.user_slash.cmp(&a.stats.user_slash),
            SortKey::Proactive => b.stats.claude_proactive.cmp(&a.stats.claude_proactive),
            SortKey::Subagent => b.stats.subagent.cmp(&a.stats.subagent),
            SortKey::LastSeen => b.stats.last_seen.cmp(&a.stats.last_seen),
        });
        rows
    }

    pub fn session_rows(&self) -> Vec<SessionEntry> {
        let Some(skill) = &self.selected_skill else {
            return Vec::new();
        };
        let owned: Vec<SkillInvocation> =
            self.windowed_invocations().into_iter().cloned().collect();
        aggregate::sessions_for_skill(&owned, skill)
            .into_iter()
            .filter(|s| {
                self.filter.is_empty()
                    || session_label(&s.session_id, &self.session_index)
                        .to_lowercase()
                        .contains(&self.filter.to_lowercase())
            })
            .collect()
    }

    pub fn session_display_label(&self, session_id: &str) -> String {
        session_label(session_id, &self.session_index)
    }

    pub fn session_git_branch(&self, session_id: &str) -> Option<String> {
        session_branch(session_id, &self.session_index)
    }

    pub fn invocation_rows(&self) -> Vec<&SkillInvocation> {
        let (Some(skill), Some(session_id)) = (&self.selected_skill, &self.selected_session) else {
            return Vec::new();
        };
        // When session-scoped, the invocation set was already parsed from
        // this session's transcripts only, and subagent records can carry
        // their own sessionId — matching on it would drop them.
        let scoped = self.session_scope.is_some();
        let mut rows: Vec<&SkillInvocation> = self
            .windowed_invocations()
            .into_iter()
            .filter(|i| &i.skill_name == skill && (scoped || &i.session_id == session_id))
            .collect();
        rows.sort_by_key(|i| i.timestamp);
        rows
    }

    /// Session-scoped per-skill counts (SessionSkills level). The invocation
    /// set is already scoped to one session, so this is the same aggregation
    /// as the global skills table over the scoped data.
    pub fn session_skill_rows(&self) -> Vec<SkillRow> {
        self.skill_rows()
    }

    /// Session-scoped flat timeline: every invocation, all skills,
    /// timestamp-ascending (SessionTimeline level).
    pub fn session_timeline_rows(&self) -> Vec<&SkillInvocation> {
        let mut rows: Vec<&SkillInvocation> = self
            .windowed_invocations()
            .into_iter()
            .filter(|i| {
                self.filter.is_empty()
                    || i.skill_name
                        .to_lowercase()
                        .contains(&self.filter.to_lowercase())
            })
            .collect();
        rows.sort_by_key(|i| i.timestamp);
        rows
    }

    /// Tab: flip between the two session-scoped presentations.
    pub fn toggle_presentation(&mut self) {
        match self.level {
            Level::SessionSkills => {
                self.level = Level::SessionTimeline;
                self.selected = 0;
            }
            Level::SessionTimeline => {
                self.level = Level::SessionSkills;
                self.selected = 0;
            }
            _ => {}
        }
    }

    pub fn current_len(&self) -> usize {
        match self.level {
            Level::Skills => self.skill_rows().len(),
            Level::Sessions => self.session_rows().len(),
            Level::Invocations => self.invocation_rows().len(),
            Level::SessionSkills => self.session_skill_rows().len(),
            Level::SessionTimeline => self.session_timeline_rows().len(),
        }
    }

    pub fn select_next(&mut self) {
        let len = self.current_len();
        if len == 0 {
            return;
        }
        self.selected = (self.selected + 1).min(len - 1);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn drill_in(&mut self) {
        match self.level {
            Level::Skills => {
                let rows = self.skill_rows();
                if let Some(row) = rows.get(self.selected) {
                    self.selected_skill = Some(row.name.clone());
                    self.level = Level::Sessions;
                    self.selected = 0;
                    self.filter.clear();
                }
            }
            Level::Sessions => {
                let rows = self.session_rows();
                if let Some(row) = rows.get(self.selected) {
                    self.selected_session = Some(row.session_id.clone());
                    self.level = Level::Invocations;
                    self.selected = 0;
                    self.filter.clear();
                }
            }
            Level::Invocations => {}
            Level::SessionSkills => {
                let rows = self.session_skill_rows();
                if let Some(row) = rows.get(self.selected) {
                    // selected_session is already pinned to the scope.
                    self.selected_skill = Some(row.name.clone());
                    self.level = Level::Invocations;
                    self.selected = 0;
                    self.filter.clear();
                }
            }
            Level::SessionTimeline => {}
        }
    }

    pub fn go_back(&mut self) {
        match self.level {
            Level::Skills => {}
            Level::Sessions => {
                self.level = Level::Skills;
                self.selected_skill = None;
                self.selected = 0;
                self.filter.clear();
            }
            Level::Invocations => {
                if self.session_scope.is_some() {
                    // Scoped drill-down came from SessionSkills; keep
                    // selected_session pinned to the scope.
                    self.level = Level::SessionSkills;
                    self.selected_skill = None;
                    self.selected = 0;
                    self.filter.clear();
                } else {
                    self.level = Level::Sessions;
                    self.selected_session = None;
                    self.selected = 0;
                    self.filter.clear();
                }
            }
            Level::SessionSkills | Level::SessionTimeline => {}
        }
    }

    pub fn cycle_sort(&mut self) {
        if matches!(self.level, Level::Skills | Level::SessionSkills) {
            self.sort_key = self.sort_key.next();
        }
    }

    pub fn cycle_time_window(&mut self) {
        self.time_window = self.time_window.next();
        self.selected = 0;
    }

    pub fn start_filter_editing(&mut self) {
        self.filter_editing = true;
    }

    pub fn stop_filter_editing(&mut self) {
        self.filter_editing = false;
        self.selected = 0;
    }

    pub fn filter_push(&mut self, c: char) {
        self.filter.push(c);
        self.selected = 0;
    }

    pub fn filter_backspace(&mut self) {
        self.filter.pop();
        self.selected = 0;
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.selected = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Origin, TriggerType};
    use chrono::TimeZone;

    fn inv(skill: &str, session: &str, hour: u32, origin: Origin) -> SkillInvocation {
        SkillInvocation {
            skill_name: skill.to_string(),
            trigger_type: TriggerType::UserSlash,
            session_id: session.to_string(),
            project_path: "/repo".to_string(),
            timestamp: Utc.with_ymd_and_hms(2026, 7, 1, hour, 0, 0).unwrap(),
            transcript_file: "/tmp/t.jsonl".to_string(),
            args: None,
            origin,
        }
    }

    fn scoped_app() -> App {
        // Scoped invocation set: main-session records plus one subagent
        // record whose sessionId differs from the scope (as in real
        // subagent transcripts).
        let invs = vec![
            inv("worktree", "sess-1", 9, Origin::Main),
            inv("worktree", "sess-1", 11, Origin::Main),
            inv("cve-lookup", "sess-1", 10, Origin::Main),
            inv("github-cli", "sess-1-sub", 12, Origin::Subagent),
        ];
        App::new_scoped(invs, SessionIndex::new(), "sess-1".to_string())
    }

    #[test]
    fn new_scoped_starts_at_session_skills_with_pinned_session() {
        let app = scoped_app();
        assert_eq!(app.level, Level::SessionSkills);
        assert_eq!(app.session_scope.as_deref(), Some("sess-1"));
        assert_eq!(app.selected_session.as_deref(), Some("sess-1"));
        assert!(app.is_scoped_top_level());
    }

    #[test]
    fn toggle_flips_between_scoped_presentations_and_resets_selection() {
        let mut app = scoped_app();
        app.selected = 1;
        app.toggle_presentation();
        assert_eq!(app.level, Level::SessionTimeline);
        assert_eq!(app.selected, 0);
        assert!(app.is_scoped_top_level());
        app.toggle_presentation();
        assert_eq!(app.level, Level::SessionSkills);
    }

    #[test]
    fn toggle_is_noop_on_global_levels() {
        let mut app = App::new(Vec::new(), SessionIndex::new());
        app.toggle_presentation();
        assert_eq!(app.level, Level::Skills);
    }

    #[test]
    fn session_timeline_rows_span_all_skills_sorted_by_timestamp() {
        let app = scoped_app();
        let rows = app.session_timeline_rows();
        let names: Vec<&str> = rows.iter().map(|i| i.skill_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["worktree", "cve-lookup", "worktree", "github-cli"]
        );
    }

    #[test]
    fn scoped_drill_in_goes_to_invocations_and_back_to_session_skills() {
        let mut app = scoped_app();
        // session_skill_rows default sort is total desc -> worktree first.
        app.drill_in();
        assert_eq!(app.level, Level::Invocations);
        assert_eq!(app.selected_skill.as_deref(), Some("worktree"));
        assert_eq!(app.invocation_rows().len(), 2);

        app.go_back();
        assert_eq!(app.level, Level::SessionSkills);
        assert!(app.selected_skill.is_none());
        // The scope pin must survive the round trip.
        assert_eq!(app.selected_session.as_deref(), Some("sess-1"));
    }

    #[test]
    fn scoped_invocation_rows_keep_subagent_records_with_foreign_session_id() {
        let mut app = scoped_app();
        app.selected_skill = Some("github-cli".to_string());
        app.level = Level::Invocations;
        // The subagent record's sessionId is "sess-1-sub", not the scope —
        // it must still appear because the data set is transcript-scoped.
        assert_eq!(app.invocation_rows().len(), 1);
    }

    #[test]
    fn go_back_and_timeline_drill_are_noops_at_scoped_top_levels() {
        let mut app = scoped_app();
        app.go_back();
        assert_eq!(app.level, Level::SessionSkills);
        app.toggle_presentation();
        app.drill_in(); // timeline is a leaf
        assert_eq!(app.level, Level::SessionTimeline);
        app.go_back();
        assert_eq!(app.level, Level::SessionTimeline);
    }

    #[test]
    fn cycle_sort_works_on_session_skills_level() {
        let mut app = scoped_app();
        assert_eq!(app.sort_key, SortKey::Total);
        app.cycle_sort();
        assert_eq!(app.sort_key, SortKey::Slash);
        // But not on the timeline presentation.
        app.toggle_presentation();
        app.cycle_sort();
        assert_eq!(app.sort_key, SortKey::Slash);
    }

    #[test]
    fn zero_invocation_scope_renders_empty_rows_everywhere() {
        let app = App::new_scoped(Vec::new(), SessionIndex::new(), "sess-empty".to_string());
        assert!(app.session_skill_rows().is_empty());
        assert!(app.session_timeline_rows().is_empty());
        assert_eq!(app.current_len(), 0);
    }
}
