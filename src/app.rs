use std::collections::HashSet;
use std::time::{Duration, Instant};

use eyre::Result;
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::layout::Rect;
use ratatui::widgets::ListState;
use tui_input::Input;

use crate::config::{MimeTuiConfig, Theme};
use crate::model::{DesktopApp, MimeType, OnDiskAssoc, PendingEdits};
use crate::storage::{self, Storage};
use crate::storage::mimeapps::{MimeConflict, SaveError, UserFileBaseline};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    ByMime,
    ByApp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Search,
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Normal browsing / inline edits.
    Browse,
    /// Modal app-picker, used from by-mime view to add an app to `for_mime`.
    PickApp { for_mime: String },
    /// Modal mime-picker, used from by-app view to associate `for_app` with a
    /// new mime type.
    PickMime { for_app: String },
    /// Esc was pressed while dirty; we're asking the user how to proceed.
    ConfirmQuit,
    /// Help overlay listing all keybindings.
    Help,
    /// Save was attempted but the user's `mimeapps.list` changed on disk
    /// since startup and at least one pending edit overlaps with that
    /// change. User picks: reload / overwrite / merge / cancel.
    ConflictResolve { conflicts: Vec<MimeConflict> },
    /// Live theme-preview picker. Carries the colours we started with so
    /// Esc can revert; `selected` is the index into `config::PRESET_NAMES`.
    ThemePick {
        original_colors: Theme,
        selected: usize,
    },
}

/// Result of `App::save`. The wrapping `Result<_, eyre::Report>` only
/// signals I/O / parse errors; user-actionable outcomes (a clean write vs
/// a conflict needing resolution) live here.
#[derive(Debug)]
pub enum SaveResult {
    Saved {
        written: usize,
        shadowed: Vec<String>,
        merged_external: bool,
    },
    Conflicts(Vec<MimeConflict>),
}

/// How an app relates to a mime in the by-app right pane. Drives display
/// markers (★ / + / ·) and which edit actions make sense for the current
/// selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// This app is the effective default for the mime.
    Default,
    /// This app is in the effective associations (declared or added), but not
    /// the default.
    Associated,
    /// The `.desktop` declares the mime via `MimeType=`, but it's been
    /// suppressed via `[Removed Associations]` (or a pending remove).
    DeclaredOnly,
}

pub struct App {
    pub view: View,
    pub focus: Focus,
    pub mode: Mode,
    pub input: Input,
    pub cursor_visible: bool,
    pub cursor_last_toggle: Instant,
    pub apps: Vec<DesktopApp>,
    pub mimes: Vec<MimeType>,
    pub assoc: OnDiskAssoc,
    /// Snapshot of the user's `mimeapps.list` at startup. Conflict detection
    /// at save time compares this against the current on-disk file to spot
    /// external edits that landed mid-session.
    pub user_baseline: UserFileBaseline,
    pub pending: PendingEdits,
    pub selected_left: usize,
    pub selected_right: usize,
    /// Horizontal scroll offset for the focused left list in by-mime /
    /// by-app views, in display columns. Long mime ids extend past the
    /// pane's width; Shift+←/→ drives this. Reset on view switch and on
    /// search-input changes.
    pub left_hscroll: u16,
    /// Same as `left_hscroll` but for the right pane's list. Some apps
    /// declare very long mime ids (`application/vnd.openxmlformats-...`)
    /// that overflow the right pane in by-app view; Shift+←/→ while
    /// Focus::Right drives this.
    pub right_hscroll: u16,
    pub left_list_state: ListState,
    pub right_list_state: ListState,
    pub left_rect: Option<Rect>,
    pub right_rect: Option<Rect>,
    pub search_rect: Option<Rect>,
    /// Picker state — only meaningful when `mode` is one of the picker variants.
    pub pick_input: Input,
    pub pick_selected: usize,
    /// Emacs-style mark: when set, the picker shows the closed range
    /// `[min(mark,sel)..=max(mark,sel)]` as a multi-selection. Space/Enter
    /// then operates on the entire range. Cleared when search input changes.
    pub pick_mark: Option<usize>,
    /// Horizontal scroll offset for the picker list, in display columns.
    /// Long mime ids overflow the picker width; this lets the user pan
    /// across with ←/→ or Ctrl-B/Ctrl-F. The relation marker (first span)
    /// stays pinned at column 0 so context isn't lost.
    pub pick_hscroll: u16,
    pub pick_list_state: ListState,
    /// Rect of the picker's candidates list, set during the picker draw
    /// so mouse handlers can hit-test clicks and scroll-wheel events.
    /// `None` between draws or when no picker is open.
    pub pick_list_rect: Option<Rect>,
    /// Row offset for the scrollable help overlay. Persists between F1
    /// openings so the user comes back where they left off; reset on
    /// dismiss.
    pub help_scroll: u16,
    /// Name of the preset currently in effect (set at startup from
    /// `config.preset`, mutated by the in-app theme picker). Used to
    /// highlight the active row in the Ctrl-T overlay.
    pub active_preset: Option<String>,
    /// Transient one-line notice (e.g. "Saved 3 edits"). Cleared after a few
    /// seconds so it doesn't accumulate.
    pub flash: Option<(String, Instant)>,
    pub config: MimeTuiConfig,
    fuzzy_matcher: SkimMatcherV2,
}

impl App {
    pub fn new(config: MimeTuiConfig) -> Result<Self> {
        let (apps, mimes, assoc) = load_world()?;
        let user_baseline = storage::mimeapps::read_user_file_baseline();
        Ok(Self {
            view: View::ByMime,
            focus: Focus::Left,
            mode: Mode::Browse,
            input: Input::default(),
            cursor_visible: true,
            cursor_last_toggle: Instant::now(),
            apps,
            mimes,
            assoc,
            user_baseline,
            pending: PendingEdits::default(),
            selected_left: 0,
            selected_right: 0,
            left_hscroll: 0,
            right_hscroll: 0,
            left_list_state: ListState::default(),
            right_list_state: ListState::default(),
            left_rect: None,
            right_rect: None,
            search_rect: None,
            pick_input: Input::default(),
            pick_selected: 0,
            pick_mark: None,
            pick_hscroll: 0,
            pick_list_state: ListState::default(),
            pick_list_rect: None,
            help_scroll: 0,
            active_preset: None,
            flash: None,
            config,
            fuzzy_matcher: SkimMatcherV2::default(),
        })
    }

    pub fn query(&self) -> String {
        self.input.value().to_string()
    }

    pub fn is_dirty(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn toggle_view(&mut self) {
        self.view = match self.view {
            View::ByMime => View::ByApp,
            View::ByApp => View::ByMime,
        };
        // The same query rarely makes sense across the two axes ("text/html"
        // matches mimes, not apps; "firefox" matches apps, not mimes), so
        // clear it on switch rather than leaving a stale filter active.
        self.input = Input::default();
        self.selected_left = 0;
        self.selected_right = 0;
        self.left_hscroll = 0;
        self.right_hscroll = 0;
        self.left_list_state.select(Some(0));
    }

    fn score(&self, haystack: &str, query: &str) -> Option<i64> {
        if query.is_empty() {
            return Some(0);
        }
        let h = haystack.to_lowercase();
        let q = query.to_lowercase();
        if h.starts_with(&q) {
            return Some(1_000_000);
        }
        self.fuzzy_matcher.fuzzy_match(&h, &q)
    }

    pub fn visible_mimes(&self) -> Vec<&MimeType> {
        let q = self.query();
        if q.is_empty() {
            return self.mimes.iter().collect();
        }
        let mut scored: Vec<(&MimeType, i64)> = self
            .mimes
            .iter()
            .filter_map(|m| {
                let by_id = self.score(&m.id, &q);
                let by_desc = self.score(&m.description, &q);
                let best = match (by_id, by_desc) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                best.map(|s| (m, s))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(m, _)| m).collect()
    }

    pub fn visible_apps(&self) -> Vec<&DesktopApp> {
        let q = self.query();
        if q.is_empty() {
            return self.apps.iter().collect();
        }
        self.apps_matching(&q)
    }

    /// Generic "search apps by query". Used by both the main app list and the
    /// picker overlay (with their own respective query inputs).
    pub fn apps_matching(&self, query: &str) -> Vec<&DesktopApp> {
        let q = query.trim();
        if q.is_empty() {
            return self.apps.iter().collect();
        }
        let mut scored: Vec<(&DesktopApp, i64)> = self
            .apps
            .iter()
            .filter_map(|a| {
                let by_name = self.score(&a.name, q);
                let by_id = self.score(&a.id, q);
                let best = match (by_name, by_id) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                best.map(|s| (a, s))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(a, _)| a).collect()
    }

    // ───────────── effective_* — fold pending edits over on-disk state ─────

    /// The default app for a mime, with pending edits applied.
    pub fn effective_default_for(&self, mime: &str) -> Option<&DesktopApp> {
        if let Some(pending) = self.pending.set_default.get(mime) {
            match pending {
                None => return None, // cleared
                Some(id) => return self.apps.iter().find(|a| &a.id == id),
            }
        }
        let id = self.assoc.defaults.get(mime)?;
        if self.is_effectively_removed(mime, id) {
            return None;
        }
        self.apps.iter().find(|a| &a.id == id)
    }

    /// True if an app is removed from a mime's associations after layering
    /// pending edits on top of on-disk state. Pending adds beat pending removes
    /// (mutators preserve this invariant).
    fn is_effectively_removed(&self, mime: &str, app_id: &str) -> bool {
        if self
            .pending
            .add
            .get(mime)
            .map(|s| s.contains(app_id))
            .unwrap_or(false)
        {
            return false;
        }
        if self
            .pending
            .remove
            .get(mime)
            .map(|s| s.contains(app_id))
            .unwrap_or(false)
        {
            return true;
        }
        self.assoc.is_removed(mime, app_id)
    }

    /// Resolved + pending associations for a mime. Order: declared via
    /// `MimeType=`, then on-disk added, then pending added (so the user's most
    /// recent adds appear at the end). Filtered through `is_effectively_removed`.
    pub fn effective_associations_for(&self, mime: &str) -> Vec<&DesktopApp> {
        let mut ids: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        for a in &self.apps {
            if a.handles(mime) && seen.insert(a.id.clone()) {
                ids.push(a.id.clone());
            }
        }
        if let Some(added) = self.assoc.added.get(mime) {
            for id in added {
                if seen.insert(id.clone()) {
                    ids.push(id.clone());
                }
            }
        }
        if let Some(added) = self.pending.add.get(mime) {
            for id in added {
                if seen.insert(id.clone()) {
                    ids.push(id.clone());
                }
            }
        }
        ids.retain(|id| !self.is_effectively_removed(mime, id));

        ids.into_iter()
            .filter_map(|id| self.apps.iter().find(|a| a.id == id))
            .collect()
    }

    /// All mimes this app has a non-trivial relationship with, in the order
    /// the by-app view should display them: defaults first, then associated,
    /// then declared-only. Within each group, sorted by mime id.
    pub fn mime_list_for_app(&self, app_id: &str) -> Vec<(MimeType, Relation)> {
        let mut defaults: Vec<MimeType> = Vec::new();
        let mut associated: Vec<MimeType> = Vec::new();
        let mut declared_only: Vec<MimeType> = Vec::new();

        for m in &self.mimes {
            let is_default = self
                .effective_default_for(&m.id)
                .map(|a| a.id == app_id)
                .unwrap_or(false);
            if is_default {
                defaults.push(m.clone());
                continue;
            }
            let is_associated = self
                .effective_associations_for(&m.id)
                .iter()
                .any(|a| a.id == app_id);
            if is_associated {
                associated.push(m.clone());
                continue;
            }
            // Declared in .desktop but suppressed by mimeapps.list.
            let declared = self
                .apps
                .iter()
                .find(|a| a.id == app_id)
                .map(|a| a.handles(&m.id))
                .unwrap_or(false);
            if declared {
                declared_only.push(m.clone());
            }
        }

        defaults.sort_by(|a, b| a.id.cmp(&b.id));
        associated.sort_by(|a, b| a.id.cmp(&b.id));
        declared_only.sort_by(|a, b| a.id.cmp(&b.id));

        let mut out: Vec<(MimeType, Relation)> = Vec::new();
        out.extend(defaults.into_iter().map(|m| (m, Relation::Default)));
        out.extend(associated.into_iter().map(|m| (m, Relation::Associated)));
        out.extend(declared_only.into_iter().map(|m| (m, Relation::DeclaredOnly)));
        out
    }

    /// The mime currently highlighted in the by-mime left list, if any.
    pub fn currently_selected_mime(&self) -> Option<&MimeType> {
        let visible = self.visible_mimes();
        visible.get(self.selected_left).copied()
    }

    pub fn currently_selected_app(&self) -> Option<&DesktopApp> {
        let visible = self.visible_apps();
        visible.get(self.selected_left).copied()
    }

    // ───────────── edit actions ────────────────────────────────────────────
    //
    // All four action helpers take an explicit (mime, app_id) so both by-mime
    // and by-app views can call them — they only differ in which side of the
    // pair comes from the selection.

    /// Set `app_id` as the default for `mime`.
    pub fn action_set_default(&mut self, mime: &str, app_id: &str) {
        self.pending.set_default(mime, Some(app_id));
        // Setting the default implies the app is associated; if the user had
        // pending-removed it, drop that.
        self.pending.add_assoc(mime, app_id);
        // …but we don't want a permanent "added" entry if the app is already
        // declared via MimeType=. Trim:
        if self
            .apps
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.handles(mime))
            .unwrap_or(false)
        {
            if let Some(set) = self.pending.add.get_mut(mime) {
                set.remove(app_id);
                if set.is_empty() {
                    self.pending.add.remove(mime);
                }
            }
        }
    }

    /// Clear the default for `mime`.
    pub fn action_clear_default(&mut self, mime: &str) {
        self.pending.set_default(mime, None);
    }

    /// Remove `app_id` from the associations of `mime`. If it was the default,
    /// clear the default too.
    pub fn action_remove_assoc(&mut self, mime: &str, app_id: &str) {
        self.pending.remove_assoc(mime, app_id);
        if self
            .effective_default_for(mime)
            .map(|d| d.id == app_id)
            .unwrap_or(false)
        {
            self.pending.set_default(mime, None);
        }
    }

    /// Add `app_id` to the associations of `mime` (used by the pickers).
    pub fn action_add_assoc(&mut self, mime: &str, app_id: &str) {
        self.pending.add_assoc(mime, app_id);
    }

    /// What relationship does `app_id` currently have to `mime`, after pending
    /// edits? `None` means no relationship at all.
    pub fn relation_of(&self, mime: &str, app_id: &str) -> Option<Relation> {
        if self
            .effective_default_for(mime)
            .map(|a| a.id == app_id)
            .unwrap_or(false)
        {
            return Some(Relation::Default);
        }
        if self
            .effective_associations_for(mime)
            .iter()
            .any(|a| a.id == app_id)
        {
            return Some(Relation::Associated);
        }
        let declared = self
            .apps
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.handles(mime))
            .unwrap_or(false);
        if declared {
            return Some(Relation::DeclaredOnly);
        }
        None
    }

    /// "Toggle on" if currently off (None or DeclaredOnly), "off" if currently
    /// active (Default or Associated).
    fn is_active_assoc(rel: Option<Relation>) -> bool {
        matches!(rel, Some(Relation::Default) | Some(Relation::Associated))
    }

    /// Discard all pending edits.
    pub fn action_discard(&mut self) {
        self.pending.clear();
    }

    /// Conflict-aware save. Returns one of:
    ///
    /// * `Ok(SaveResult::Saved { written, shadowed, merged_external })` —
    ///   wrote successfully. `shadowed` lists mimes whose new default will
    ///   be silently overridden by a higher-priority per-desktop file.
    ///   `merged_external` is `true` if non-conflicting external edits
    ///   landed mid-session and were preserved.
    /// * `Ok(SaveResult::Conflicts(c))` — external changes overlap with our
    ///   pending edits; nothing was written. The caller should route to
    ///   `Mode::ConflictResolve` so the user can choose how to proceed.
    /// * `Err(_)` — I/O / parse failure.
    pub fn save(&mut self) -> Result<SaveResult> {
        let path = storage::mimeapps::user_mimeapps_path()
            .ok_or_else(|| eyre::eyre!("could not resolve XDG_CONFIG_HOME"))?;
        let pending_defaults: Vec<String> =
            self.pending.set_default.keys().cloned().collect();

        match storage::mimeapps::save_user_file_safely_at(
            &path,
            &self.pending,
            &self.user_baseline,
        ) {
            Ok(outcome) => {
                let merged = outcome.merged_external_changes;
                let written = outcome.written;
                self.finalise_save();
                let shadowed = self.compute_shadowed(&pending_defaults);
                let _ = storage::mimeapps::run_update_desktop_database();
                Ok(SaveResult::Saved {
                    written,
                    shadowed,
                    merged_external: merged,
                })
            }
            Err(SaveError::Conflicts(conflicts)) => Ok(SaveResult::Conflicts(conflicts)),
            Err(SaveError::Io(e)) => Err(e),
        }
    }

    /// Force-write pending over whatever's on disk — used by the
    /// ConflictResolve "overwrite" action. Skips conflict detection;
    /// external edits to overlapping mimes get clobbered.
    pub fn save_force(&mut self) -> Result<(usize, Vec<String>)> {
        let path = storage::mimeapps::user_mimeapps_path()
            .ok_or_else(|| eyre::eyre!("could not resolve XDG_CONFIG_HOME"))?;
        let written = self.pending.count();
        let pending_defaults: Vec<String> =
            self.pending.set_default.keys().cloned().collect();
        storage::mimeapps::save_user_file_force_at(&path, &self.pending)?;
        self.finalise_save();
        let shadowed = self.compute_shadowed(&pending_defaults);
        let _ = storage::mimeapps::run_update_desktop_database();
        Ok((written, shadowed))
    }

    /// Discard the conflicting pending edits and try the save again. Used
    /// by the ConflictResolve "merge" action.
    pub fn save_dropping_conflicts(
        &mut self,
        conflicts: &[MimeConflict],
    ) -> Result<SaveResult> {
        storage::mimeapps::drop_conflicting_edits(&mut self.pending, conflicts);
        self.save()
    }

    /// Drop all pending edits and re-read the on-disk state as if the user
    /// had just opened mime-tui. Used by the ConflictResolve "reload"
    /// action.
    pub fn reload_from_disk(&mut self) {
        self.pending.clear();
        self.assoc = storage::mimeapps::read_all();
        self.user_baseline = storage::mimeapps::read_user_file_baseline();
    }

    fn finalise_save(&mut self) {
        self.pending.clear();
        self.assoc = storage::mimeapps::read_all();
        self.user_baseline = storage::mimeapps::read_user_file_baseline();
    }

    fn compute_shadowed(&self, pending_defaults: &[String]) -> Vec<String> {
        let user_path = storage::mimeapps::user_mimeapps_path();
        pending_defaults
            .iter()
            .filter(|mime| {
                let actual_source = self.assoc.default_sources.get(*mime);
                match (actual_source, user_path.as_ref()) {
                    (Some(src), Some(up)) => src != up,
                    _ => false,
                }
            })
            .cloned()
            .collect()
    }

    // ───────────── picker ──────────────────────────────────────────────────

    pub fn open_pick_app(&mut self, for_mime: String) {
        self.mode = Mode::PickApp { for_mime };
        self.pick_input = Input::default();
        self.pick_selected = 0;
        self.pick_mark = None;
        self.pick_hscroll = 0;
        self.pick_list_state.select(Some(0));
    }

    pub fn open_pick_mime(&mut self, for_app: String) {
        self.mode = Mode::PickMime { for_app };
        self.pick_input = Input::default();
        self.pick_selected = 0;
        self.pick_mark = None;
        self.pick_hscroll = 0;
        self.pick_list_state.select(Some(0));
    }

    pub fn close_picker(&mut self) {
        self.mode = Mode::Browse;
        self.pick_mark = None;
        self.pick_hscroll = 0;
    }

    // ───────────── theme picker (Ctrl-T) ───────────────────────────────────

    /// Open the live theme-preview overlay. Snapshots the current colours
    /// so Esc can revert if the user is just browsing.
    pub fn open_theme_pick(&mut self) {
        let original_colors = self.config.colors.clone();
        // Start the cursor on something sensible — the user's most recent
        // active preset if known, otherwise the first entry.
        let selected = crate::config::PRESET_NAMES
            .iter()
            .position(|n| Some(*n) == self.active_preset.as_deref())
            .unwrap_or(0);
        self.mode = Mode::ThemePick {
            original_colors,
            selected,
        };
    }

    /// Apply the preset at the given index for live preview. Updates the
    /// resolved Theme in `self.config.colors` so the next frame paints with
    /// it; also writes back the selected index so the overlay knows what to
    /// highlight.
    pub fn preview_theme(&mut self, idx: usize) {
        let name = match crate::config::PRESET_NAMES.get(idx) {
            Some(n) => *n,
            None => return,
        };
        crate::config::apply_preset(&mut self.config, name);
        if let Mode::ThemePick { selected, .. } = &mut self.mode {
            *selected = idx;
        }
        self.active_preset = Some(name.to_string());
    }

    /// Close the theme picker. `cancel = true` reverts to the colours we
    /// snapshotted on open; `false` keeps the current preview.
    pub fn close_theme_pick(&mut self, cancel: bool) {
        if cancel {
            if let Mode::ThemePick { original_colors, .. } = self.mode.clone() {
                self.config.colors = original_colors;
            }
        }
        self.mode = Mode::Browse;
    }

    /// Set the mark at the current cursor if there isn't one; clear it
    /// otherwise. Matches the bottom-bar hint, which switches between "set
    /// mark" and "clear mark" based on the same condition.
    pub fn picker_toggle_mark(&mut self) {
        if self.pick_mark.is_some() {
            self.pick_mark = None;
        } else {
            self.pick_mark = Some(self.pick_selected);
        }
    }

    /// Extract (mime, app_id) for the picker row at index `i` in the current
    /// visible list. View-aware.
    fn picker_pair_at(&self, i: usize) -> Option<(String, String)> {
        match self.mode.clone() {
            Mode::PickApp { for_mime } => {
                let candidates = self.picker_visible_apps();
                let a = candidates.get(i)?;
                Some((for_mime, a.id.clone()))
            }
            Mode::PickMime { for_app } => {
                let candidates = self.picker_visible_mimes();
                let m = candidates.get(i)?;
                Some((m.id.clone(), for_app))
            }
            _ => None,
        }
    }

    /// Apply the picker's toggle action — single row if no mark, otherwise the
    /// whole marked range. For a range, uses uniform semantics: if any row is
    /// currently off, turn them all on; if all are on, turn all off.
    pub fn picker_apply_toggle(&mut self) {
        let range: Vec<usize> = match self.pick_mark {
            None => vec![self.pick_selected],
            Some(mark) => {
                let lo = mark.min(self.pick_selected);
                let hi = mark.max(self.pick_selected);
                (lo..=hi).collect()
            }
        };

        let pairs: Vec<(String, String)> = range
            .iter()
            .filter_map(|i| self.picker_pair_at(*i))
            .collect();
        if pairs.is_empty() {
            return;
        }

        let any_off = pairs
            .iter()
            .any(|(m, a)| !Self::is_active_assoc(self.relation_of(m, a)));

        let mut changed = 0usize;
        for (mime, app_id) in &pairs {
            let active = Self::is_active_assoc(self.relation_of(mime, app_id));
            if any_off && !active {
                self.action_add_assoc(mime, app_id);
                changed += 1;
            } else if !any_off && active {
                self.action_remove_assoc(mime, app_id);
                changed += 1;
            }
        }

        let verb = if any_off { "Associated" } else { "Removed" };
        let label = match self.mode.clone() {
            Mode::PickApp { for_mime } => format!("with {}", for_mime),
            Mode::PickMime { for_app } => format!("from {}", for_app),
            _ => String::new(),
        };
        self.set_flash(format!(
            "{} {} item{} {}",
            verb,
            changed,
            if changed == 1 { "" } else { "s" },
            label
        ));

        self.pick_mark = None;
    }

    pub fn picker_visible_apps(&self) -> Vec<&DesktopApp> {
        self.apps_matching(self.pick_input.value())
    }

    pub fn picker_visible_mimes(&self) -> Vec<&MimeType> {
        let q = self.pick_input.value().trim();
        if q.is_empty() {
            return self.mimes.iter().collect();
        }
        let mut scored: Vec<(&MimeType, i64)> = self
            .mimes
            .iter()
            .filter_map(|m| {
                let by_id = self.score(&m.id, q);
                let by_desc = self.score(&m.description, q);
                let best = match (by_id, by_desc) {
                    (Some(a), Some(b)) => Some(a.max(b)),
                    (Some(a), None) | (None, Some(a)) => Some(a),
                    (None, None) => None,
                };
                best.map(|s| (m, s))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(m, _)| m).collect()
    }

    // ───────────── transient flash ─────────────────────────────────────────

    pub fn set_flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), Instant::now()));
    }

    pub fn flash_message(&self) -> Option<&str> {
        match &self.flash {
            Some((msg, ts)) if ts.elapsed() < Duration::from_secs(3) => Some(msg.as_str()),
            _ => None,
        }
    }

    // ───────────── cursor blink (unchanged from Phase 1) ───────────────────

    pub fn update_cursor_blink(&mut self) {
        let blink_interval = self.config.colors.cursor_blink_interval;
        if blink_interval == 0 {
            self.cursor_visible = true;
            return;
        }
        if self.cursor_last_toggle.elapsed() >= Duration::from_millis(blink_interval) {
            self.cursor_visible = !self.cursor_visible;
            self.cursor_last_toggle = Instant::now();
        }
    }

    pub fn reset_cursor_blink(&mut self) {
        self.cursor_visible = true;
        self.cursor_last_toggle = Instant::now();
    }
}

impl App {
    /// Construct an App with caller-supplied data, no filesystem I/O. Tests
    /// only — production goes through `App::new`.
    #[cfg(test)]
    pub(crate) fn for_test(
        apps: Vec<DesktopApp>,
        mimes: Vec<MimeType>,
        assoc: OnDiskAssoc,
    ) -> Self {
        Self {
            view: View::ByMime,
            focus: Focus::Left,
            mode: Mode::Browse,
            input: Input::default(),
            cursor_visible: true,
            cursor_last_toggle: Instant::now(),
            user_baseline: UserFileBaseline::default(),
            apps,
            mimes,
            assoc,
            pending: PendingEdits::default(),
            selected_left: 0,
            selected_right: 0,
            left_hscroll: 0,
            right_hscroll: 0,
            left_list_state: ListState::default(),
            right_list_state: ListState::default(),
            left_rect: None,
            right_rect: None,
            search_rect: None,
            pick_input: Input::default(),
            pick_selected: 0,
            pick_mark: None,
            pick_hscroll: 0,
            pick_list_state: ListState::default(),
            pick_list_rect: None,
            help_scroll: 0,
            active_preset: None,
            flash: None,
            config: MimeTuiConfig::default(),
            fuzzy_matcher: SkimMatcherV2::default(),
        }
    }
}

/// Build the full world: parse `.desktop` files, read mimeapps.list, resolve
/// shared-mime-info descriptions for the mime universe we care about.
fn load_world() -> Result<(Vec<DesktopApp>, Vec<MimeType>, OnDiskAssoc)> {
    let mut storage = Storage::open()?;

    let dirs = storage::desktop::xdg_application_dirs();
    storage::desktop::refresh_app_cache(&mut storage.conn, &dirs)?;
    let apps = storage::desktop::load_apps(&storage.conn)?;

    let assoc = storage::mimeapps::read_all();

    let mut universe: HashSet<String> = HashSet::new();
    for a in &apps {
        for m in &a.mime_types {
            universe.insert(m.clone());
        }
    }
    for k in assoc.defaults.keys().chain(assoc.added.keys()).chain(assoc.removed.keys()) {
        universe.insert(k.clone());
    }

    let mime_ids: Vec<String> = universe.into_iter().collect();
    storage::mime_info::refresh_descriptions(&mut storage.conn, &mime_ids)?;
    let descriptions = storage::mime_info::load_descriptions(&storage.conn)?;

    let mut mimes: Vec<MimeType> = mime_ids
        .into_iter()
        .map(|id| {
            let description = descriptions.get(&id).cloned().unwrap_or_default();
            MimeType { id, description }
        })
        .collect();
    mimes.sort_by(|a, b| a.id.cmp(&b.id));

    Ok((apps, mimes, assoc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_world() -> (Vec<DesktopApp>, Vec<MimeType>, OnDiskAssoc) {
        let apps = vec![
            DesktopApp {
                id: "firefox.desktop".into(),
                name: "Firefox".into(),
                comment: "".into(),
                exec: "firefox".into(),
                terminal: false,
                mime_types: vec!["text/html".into()],
                category: "Network".into(),
            },
            DesktopApp {
                id: "chromium.desktop".into(),
                name: "Chromium".into(),
                comment: "".into(),
                exec: "chromium".into(),
                terminal: false,
                mime_types: vec!["text/html".into()],
                category: "Network".into(),
            },
        ];
        let mimes = vec![MimeType {
            id: "text/html".into(),
            description: "HTML document".into(),
        }];
        (apps, mimes, OnDiskAssoc::default())
    }

    #[test]
    fn set_default_records_pending_and_marks_dirty() {
        let (apps, mimes, assoc) = sample_world();
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_set_default("text/html", "firefox.desktop");
        assert!(app.is_dirty());
        assert_eq!(app.pending.count(), 1);
        // effective_default_for reflects it immediately.
        assert_eq!(
            app.effective_default_for("text/html").map(|a| a.id.as_str()),
            Some("firefox.desktop"),
        );
    }

    #[test]
    fn remove_assoc_filters_from_effective_view() {
        let (apps, mimes, assoc) = sample_world();
        let mut app = App::for_test(apps, mimes, assoc);
        assert_eq!(app.effective_associations_for("text/html").len(), 2);
        app.action_remove_assoc("text/html", "firefox.desktop");
        let remaining: Vec<&str> = app
            .effective_associations_for("text/html")
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(remaining, vec!["chromium.desktop"]);
    }

    #[test]
    fn remove_then_add_clears_remove() {
        let (apps, mimes, assoc) = sample_world();
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_remove_assoc("text/html", "firefox.desktop");
        app.action_add_assoc("text/html", "firefox.desktop");
        // Both add and remove for the same (mime, id) is an inconsistent
        // pending state; mutators must keep only one.
        assert!(
            !app.pending
                .remove
                .get("text/html")
                .map(|s| s.contains("firefox.desktop"))
                .unwrap_or(false),
            "add should clear an inverse remove"
        );
        assert_eq!(app.effective_associations_for("text/html").len(), 2);
    }

    #[test]
    fn set_default_includes_app_in_associations() {
        // Even if the app doesn't declare MimeType=text/html, setting it as
        // the default should make it appear in the effective associations.
        let mut apps = vec![DesktopApp {
            id: "outlier.desktop".into(),
            name: "Outlier".into(),
            comment: "".into(),
            exec: "outlier".into(),
            terminal: false,
            mime_types: vec![], // doesn't declare text/html
            category: "Utilities".into(),
        }];
        apps.extend(sample_world().0);
        let (_, mimes, assoc) = sample_world();
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_set_default("text/html", "outlier.desktop");
        assert_eq!(
            app.effective_default_for("text/html").map(|a| a.id.as_str()),
            Some("outlier.desktop"),
        );
        let assoc_ids: Vec<&str> = app
            .effective_associations_for("text/html")
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert!(assoc_ids.contains(&"outlier.desktop"));
    }

    #[test]
    fn save_end_to_end() {
        use std::fs;

        let dir = std::env::temp_dir().join(format!(
            "mime_tui_app_save_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("mimeapps.list");

        let (apps, mimes, assoc) = sample_world();
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_set_default("text/html", "firefox.desktop");
        app.action_add_assoc("text/html", "chromium.desktop"); // already declared, dedup

        // Drive the save explicitly through the storage layer so we can target
        // the temp path. (App::save() uses XDG_CONFIG_HOME which we don't
        // want to mutate during tests.)
        crate::storage::mimeapps::save_user_file_at(&path, &app.pending).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("[Default Applications]"));
        assert!(content.contains("text/html=firefox.desktop;"));
        // chromium.desktop is already-declared, but we did add it to pending —
        // it will land in [Added Associations]. Either presence (or none) is
        // acceptable; this assertion captures current behaviour.
        assert!(content.contains("text/html=chromium.desktop;"));

        let _ = fs::remove_dir_all(&dir);
    }
}
