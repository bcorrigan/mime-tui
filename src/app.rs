use std::collections::{BTreeSet, HashSet};
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
    /// Pre-save review modal. Lists every pending edit grouped by mime;
    /// Enter / y commits via `do_save`, Esc / n / q returns to Browse with
    /// edits intact.
    ConfirmSave,
}

/// How a single mime's default is changing. Used by the confirm-save modal
/// to render a one-line summary per default change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultChange {
    /// Default is being set to `new`. `old` is the on-disk default if any.
    Set { new: String, old: Option<String> },
    /// Default is being cleared. `old` is the on-disk default if any (None
    /// in the rare case the user clears an already-unset default).
    Cleared { old: Option<String> },
}

/// A mime association pointing at a `.desktop` that isn't installed.
/// Surfaced in the by-mime right pane so the user can see — and clean
/// up — stale entries left behind by uninstalled apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingAssoc {
    pub app_id: String,
    /// True when this is (or would be) the default app for the mime,
    /// driving the ★ marker on the row.
    pub is_default: bool,
    /// True when the user has already marked this id for removal.
    pub is_pending_removed: bool,
}

/// All pending edits affecting one mime, in a shape the confirm-save UI
/// can iterate over directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSummary {
    pub mime: String,
    pub default_change: Option<DefaultChange>,
    /// Sorted app ids being added to this mime's associations.
    pub adds: Vec<String>,
    /// Sorted app ids being removed.
    pub removes: Vec<String>,
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
    /// Vertical scroll offset for the confirm-save modal. Reset to 0 each
    /// time the modal opens so the user starts at the top.
    pub confirm_save_vscroll: u16,
    /// Horizontal scroll offset for the confirm-save modal. Long mime ids
    /// can overflow the modal width; Shift+←/→ moves this.
    pub confirm_save_hscroll: u16,
    /// Name of the preset currently in effect (set at startup from
    /// `config.preset`, mutated by the in-app theme picker). Used to
    /// highlight the active row in the Ctrl-T overlay.
    pub active_preset: Option<String>,
    /// Transient one-line notice (e.g. "Saved 3 edits"). Cleared after a few
    /// seconds so it doesn't accumulate.
    pub flash: Option<(String, Instant)>,
    /// Set by Ctrl-Z; consumed by the main loop, which tears down the TUI
    /// state, raises SIGTSTP to suspend the process, and re-arms the TUI on
    /// resume. Kept as a flag rather than handled inline in `events.rs` so
    /// terminal setup/teardown stays centralised in `main.rs`.
    pub pending_suspend: bool,
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
            confirm_save_vscroll: 0,
            confirm_save_hscroll: 0,
            active_preset: None,
            flash: None,
            pending_suspend: false,
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

    /// Remove `app_id` from the associations of `mime`. If it was the
    /// default, also clear the default — leaving `[Default Applications]
    /// X=mime` while `[Removed Associations] X=mime` is set is a
    /// "valid-file, useless-state" combination that no spec-compliant
    /// resolver would honour anyway.
    ///
    /// Returns `true` when the call also cleared a (would-be) default —
    /// callers use this to surface an explanatory flash.
    ///
    /// Special-cases the "added then removed, all in this session" pattern:
    /// if the row only exists because of a pending add (no on-disk anchor),
    /// we cancel the pending add instead of writing a remove entry.
    pub fn action_remove_assoc(&mut self, mime: &str, app_id: &str) -> bool {
        if !self.is_on_disk(mime, app_id) {
            // Pure pending-add row. Drop the add and any pending default
            // that pointed at it.
            self.pending.cancel_add(mime, app_id);
            if let Some(Some(id)) = self.pending.set_default.get(mime) {
                if id == app_id {
                    self.pending.set_default.remove(mime);
                    return true;
                }
            }
            return false;
        }

        // Snapshot the would-be default BEFORE mutating pending.remove,
        // otherwise the row we're about to suppress would mask itself out
        // of `effective_default_for` and this check never fires (the
        // historical bug that left orphan defaults behind).
        let was_default = self.would_be_default_id(mime).as_deref() == Some(app_id);
        self.pending.remove_assoc(mime, app_id);
        if !was_default {
            return false;
        }

        // Cascade: drop any pending default change targeting this app,
        // and if the on-disk default also pointed here, explicitly clear
        // it so the save doesn't preserve the dangling line.
        let pending_default_was_app = matches!(
            self.pending.set_default.get(mime),
            Some(Some(id)) if id == app_id
        );
        if pending_default_was_app {
            self.pending.set_default.remove(mime);
        }
        let on_disk_default_was_app = self
            .assoc
            .defaults
            .get(mime)
            .map(|s| s == app_id)
            .unwrap_or(false);
        if on_disk_default_was_app && !self.pending.set_default.contains_key(mime) {
            self.pending.set_default(mime, None);
        }
        true
    }

    /// True if `app_id` has *any* on-disk presence for `mime` — declared in
    /// its .desktop's `MimeType=`, listed in `[Added Associations]`, or
    /// recorded as the default — and isn't currently suppressed by the
    /// `[Removed Associations]` block. This is the anchor that decides
    /// whether removing the row needs a `pending.remove` entry or can be
    /// expressed as a pure cancellation of a pending add.
    fn is_on_disk(&self, mime: &str, app_id: &str) -> bool {
        if self.assoc.is_removed(mime, app_id) {
            return false;
        }
        let declared = self
            .apps
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.handles(mime))
            .unwrap_or(false);
        let on_disk_added = self
            .assoc
            .added
            .get(mime)
            .map(|s| s.iter().any(|i| i == app_id))
            .unwrap_or(false);
        let on_disk_default = self
            .assoc
            .defaults
            .get(mime)
            .map(|d| d == app_id)
            .unwrap_or(false);
        declared || on_disk_added || on_disk_default
    }

    /// Add `app_id` to the associations of `mime` (used by the pickers).
    pub fn action_add_assoc(&mut self, mime: &str, app_id: &str) {
        self.pending.add_assoc(mime, app_id);
    }

    /// Reverse a pending removal — the inverse of `action_remove_assoc` for
    /// `r`'s toggle path. Doesn't add to `pending.add`: if the row was on
    /// disk to begin with, this returns it to a fully clean state.
    pub fn action_undo_remove(&mut self, mime: &str, app_id: &str) {
        self.pending.undo_remove(mime, app_id);
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

    /// True when the (mime, app) row in the right pane reflects an unsaved
    /// edit. Used by the by-mime / by-app right-pane renderers to bold the
    /// row and stamp it with a pending sigil.
    ///
    /// Four cases produce "pending":
    /// 1. The app is in `pending.add[mime]` — a newly-added association.
    /// 2. The app is in `pending.remove[mime]` — being removed (the row
    ///    still renders, with strikethrough).
    /// 3. The app is the pending new default for the mime
    ///    (`pending.set_default[mime] == Some(Some(app_id))`).
    /// 4. The app *was* the on-disk default for the mime AND the default is
    ///    being changed/cleared (`pending.set_default` has any entry for
    ///    that mime) — i.e. this row is losing its default.
    pub fn is_pending_row(&self, mime: &str, app_id: &str) -> bool {
        if self
            .pending
            .add
            .get(mime)
            .map(|s| s.contains(app_id))
            .unwrap_or(false)
        {
            return true;
        }
        if self.is_pending_removed_row(mime, app_id) {
            return true;
        }
        if let Some(slot) = self.pending.set_default.get(mime) {
            // New default goes bold.
            if matches!(slot, Some(id) if id == app_id) {
                return true;
            }
            // Old default also goes bold — its star is being taken away.
            if let Some(old) = self.assoc.defaults.get(mime) {
                if old == app_id {
                    return true;
                }
            }
        }
        false
    }

    /// True when this (mime, app) row is marked for removal. Used by the
    /// UI to apply the strikethrough modifier on top of the regular pending
    /// styling.
    pub fn is_pending_removed_row(&self, mime: &str, app_id: &str) -> bool {
        self.pending
            .remove
            .get(mime)
            .map(|s| s.contains(app_id))
            .unwrap_or(false)
    }

    /// What relation `app_id` would have to `mime` if `pending.remove` didn't
    /// exist. Mirrors `relation_of` but ignores pending removals so the
    /// "displayable" lists can show what the user is *about to* remove with
    /// its original marker (★ / ✓ / ·) still intact.
    fn relation_excluding_remove(&self, mime: &str, app_id: &str) -> Option<Relation> {
        // Default check — same precedence as `effective_default_for`, but the
        // `is_effectively_removed` filter (which honours pending.remove) is
        // intentionally skipped.
        let default = if let Some(slot) = self.pending.set_default.get(mime) {
            slot.as_deref()
        } else {
            self.assoc.defaults.get(mime).map(String::as_str)
        };
        if default == Some(app_id) {
            return Some(Relation::Default);
        }

        let declared = self
            .apps
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.handles(mime))
            .unwrap_or(false);
        let on_disk_added = self
            .assoc
            .added
            .get(mime)
            .map(|s| s.iter().any(|i| i == app_id))
            .unwrap_or(false);
        let pending_added = self
            .pending
            .add
            .get(mime)
            .map(|s| s.contains(app_id))
            .unwrap_or(false);
        // The on-disk `[Removed Associations]` block still suppresses rows
        // — those are permanently removed from the user's view, unrelated
        // to the current session's pending.remove. Pending re-adds beat it.
        let suppressed = self.assoc.is_removed(mime, app_id) && !pending_added;

        if (declared || on_disk_added || pending_added) && !suppressed {
            return Some(Relation::Associated);
        }
        if declared {
            return Some(Relation::DeclaredOnly);
        }
        None
    }

    /// Like `effective_associations_for`, but also includes apps that are
    /// `pending.remove`d for this mime, paired with a flag so the UI can
    /// render them with strikethrough. Apps still suppressed by the on-disk
    /// `[Removed Associations]` block remain filtered out.
    pub fn displayable_associations_for(
        &self,
        mime: &str,
    ) -> Vec<(&DesktopApp, bool)> {
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
        // Surface pending removals — these are the rows we want to render
        // with strikethrough so the user sees what they're about to lose.
        if let Some(removed) = self.pending.remove.get(mime) {
            for id in removed {
                if seen.insert(id.clone()) {
                    ids.push(id.clone());
                }
            }
        }
        // Drop apps suppressed by the on-disk removed list (unless pending
        // re-adds them). This is the persistent "user removed in a previous
        // session" state — not session-local.
        ids.retain(|id| {
            if self
                .pending
                .add
                .get(mime)
                .map(|s| s.contains(id))
                .unwrap_or(false)
            {
                return true;
            }
            !self.assoc.is_removed(mime, id)
        });

        ids.into_iter()
            .filter_map(|id| {
                let app = self.apps.iter().find(|a| a.id == id)?;
                let removed = self.is_pending_removed_row(mime, &id);
                Some((app, removed))
            })
            .collect()
    }

    /// Apps referenced by this mime's on-disk or pending state but not
    /// present in the installed apps index — i.e. phantom entries left
    /// behind by uninstalled `.desktop` files. The by-mime right pane
    /// renders these in the `invalid` theme colour so the user can see
    /// what to clean up.
    pub fn missing_associations_for(&self, mime: &str) -> Vec<MissingAssoc> {
        let mut ids: BTreeSet<String> = BTreeSet::new();
        if let Some(id) = self.assoc.defaults.get(mime) {
            ids.insert(id.clone());
        }
        if let Some(added) = self.assoc.added.get(mime) {
            for id in added {
                ids.insert(id.clone());
            }
        }
        if let Some(added) = self.pending.add.get(mime) {
            for id in added {
                ids.insert(id.clone());
            }
        }
        if let Some(Some(id)) = self.pending.set_default.get(mime) {
            ids.insert(id.clone());
        }
        if let Some(removed) = self.pending.remove.get(mime) {
            for id in removed {
                ids.insert(id.clone());
            }
        }

        let default_id = self.would_be_default_id(mime);
        ids.into_iter()
            .filter(|id| !self.apps.iter().any(|a| &a.id == id))
            .map(|id| {
                let is_default = default_id.as_deref() == Some(&id);
                let is_pending_removed = self.is_pending_removed_row(mime, &id);
                MissingAssoc {
                    app_id: id,
                    is_default,
                    is_pending_removed,
                }
            })
            .collect()
    }

    /// The app id that *would* be the default for `mime` if we ignored
    /// whether the app is installed. Pending overrides win, then on-disk;
    /// `[Removed Associations]` and an explicit pending "clear default"
    /// both yield `None`.
    fn would_be_default_id(&self, mime: &str) -> Option<String> {
        if let Some(slot) = self.pending.set_default.get(mime) {
            return slot.clone();
        }
        let on_disk = self.assoc.defaults.get(mime)?;
        if self.assoc.is_removed(mime, on_disk) {
            return None;
        }
        Some(on_disk.clone())
    }

    /// Short-circuiting predicate: does this mime have *any* missing
    /// association (default, added, or pending edit pointing at a
    /// `.desktop` that isn't installed)? Drives the red row tint in the
    /// by-mime left pane.
    pub fn mime_has_missing(&self, mime: &str) -> bool {
        let installed = |id: &str| self.apps.iter().any(|a| a.id == id);
        let is_phantom = |id: &str| !installed(id);

        if let Some(id) = self.assoc.defaults.get(mime) {
            if is_phantom(id) {
                return true;
            }
        }
        if let Some(added) = self.assoc.added.get(mime) {
            if added.iter().any(|id| is_phantom(id)) {
                return true;
            }
        }
        if let Some(added) = self.pending.add.get(mime) {
            if added.iter().any(|id| is_phantom(id)) {
                return true;
            }
        }
        if let Some(Some(id)) = self.pending.set_default.get(mime) {
            if is_phantom(id) {
                return true;
            }
        }
        if let Some(removed) = self.pending.remove.get(mime) {
            if removed.iter().any(|id| is_phantom(id)) {
                return true;
            }
        }
        false
    }

    /// If the (would-be) default for `mime` is a non-installed app,
    /// return its id. Used by the by-mime detail pane to render
    /// `X.desktop (not installed)` in invalid styling rather than
    /// silently showing "(none set)".
    pub fn missing_default_for(&self, mime: &str) -> Option<String> {
        let id = self.would_be_default_id(mime)?;
        if self.apps.iter().any(|a| a.id == id) {
            return None;
        }
        Some(id)
    }

    /// Like `mime_list_for_app`, but also includes mimes the user has
    /// pending-removed for this app — paired with the "would-be" relation
    /// (what the marker would have shown before the remove) and a
    /// pending-removed flag.
    pub fn displayable_mime_list_for_app(
        &self,
        app_id: &str,
    ) -> Vec<(MimeType, Relation, bool)> {
        let mut defaults: Vec<(MimeType, bool)> = Vec::new();
        let mut associated: Vec<(MimeType, bool)> = Vec::new();
        let mut declared_only: Vec<(MimeType, bool)> = Vec::new();

        for m in &self.mimes {
            let removed = self.is_pending_removed_row(&m.id, app_id);
            let rel = self.relation_excluding_remove(&m.id, app_id);
            match rel {
                Some(Relation::Default) => defaults.push((m.clone(), removed)),
                Some(Relation::Associated) => associated.push((m.clone(), removed)),
                Some(Relation::DeclaredOnly) => declared_only.push((m.clone(), removed)),
                None => {}
            }
        }

        defaults.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        associated.sort_by(|a, b| a.0.id.cmp(&b.0.id));
        declared_only.sort_by(|a, b| a.0.id.cmp(&b.0.id));

        let mut out = Vec::new();
        out.extend(defaults.into_iter().map(|(m, r)| (m, Relation::Default, r)));
        out.extend(
            associated
                .into_iter()
                .map(|(m, r)| (m, Relation::Associated, r)),
        );
        out.extend(
            declared_only
                .into_iter()
                .map(|(m, r)| (m, Relation::DeclaredOnly, r)),
        );
        out
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
    /// Enter the confirm-save review modal. Scroll resets each time so the
    /// user lands at the top of the change list.
    pub fn open_confirm_save(&mut self) {
        self.confirm_save_vscroll = 0;
        self.confirm_save_hscroll = 0;
        self.mode = Mode::ConfirmSave;
    }

    /// Leave the confirm-save modal back to Browse without saving.
    pub fn close_confirm_save(&mut self) {
        self.mode = Mode::Browse;
    }

    /// Build a per-mime summary of every pending edit, suitable for direct
    /// rendering by the confirm-save modal. Mimes are returned sorted; adds
    /// and removes within each are also sorted (alphabetic on app id). The
    /// goal is a stable, glanceable view of the diff the user is about to
    /// commit.
    pub fn pending_change_summary(&self) -> Vec<ChangeSummary> {
        use std::collections::BTreeSet;

        let mut mimes: BTreeSet<String> = BTreeSet::new();
        mimes.extend(self.pending.set_default.keys().cloned());
        mimes.extend(self.pending.add.keys().cloned());
        mimes.extend(self.pending.remove.keys().cloned());

        let mut out = Vec::with_capacity(mimes.len());
        for mime in mimes {
            let default_change = self.pending.set_default.get(&mime).map(|slot| {
                let old = self.assoc.defaults.get(&mime).cloned();
                match slot {
                    Some(new) => DefaultChange::Set {
                        new: new.clone(),
                        old,
                    },
                    None => DefaultChange::Cleared { old },
                }
            });
            let mut adds: Vec<String> = self
                .pending
                .add
                .get(&mime)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            adds.sort();
            let mut removes: Vec<String> = self
                .pending
                .remove
                .get(&mime)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            removes.sort();

            out.push(ChangeSummary {
                mime,
                default_change,
                adds,
                removes,
            });
        }
        out
    }

    /// Display name for an app id, falling back to the id itself when the
    /// app isn't in our index (e.g. the .desktop has been uninstalled
    /// between when the edit was queued and when we render).
    pub fn app_name_for(&self, app_id: &str) -> String {
        self.apps
            .iter()
            .find(|a| a.id == app_id)
            .map(|a| a.name.clone())
            .unwrap_or_else(|| app_id.to_string())
    }

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
            confirm_save_vscroll: 0,
            confirm_save_hscroll: 0,
            active_preset: None,
            flash: None,
            pending_suspend: false,
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
    fn is_pending_row_flags_pending_add() {
        let (apps, mimes, mut assoc) = sample_world();
        // Treat firefox as a fresh app that *doesn't* declare text/html on
        // disk, so action_add_assoc actually lands in pending.add (not a
        // no-op against the declared-only baseline).
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("chromium.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);
        // Remove firefox from declarations so add is unambiguous.
        app.apps[0].mime_types.clear();
        app.action_add_assoc("text/html", "firefox.desktop");
        assert!(app.is_pending_row("text/html", "firefox.desktop"));
        // chromium is on disk, no pending edit → not pending.
        assert!(!app.is_pending_row("text/html", "chromium.desktop"));
    }

    #[test]
    fn is_pending_row_flags_new_and_old_default_on_default_change() {
        let (apps, mimes, mut assoc) = sample_world();
        // On-disk default: firefox.
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_set_default("text/html", "chromium.desktop");
        // chromium becomes the new default → pending.
        assert!(app.is_pending_row("text/html", "chromium.desktop"));
        // firefox loses default → also pending (its star is going away).
        assert!(app.is_pending_row("text/html", "firefox.desktop"));
    }

    #[test]
    fn is_pending_row_flags_old_default_on_clear() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_clear_default("text/html");
        assert!(app.is_pending_row("text/html", "firefox.desktop"));
        // Unrelated app is unaffected.
        assert!(!app.is_pending_row("text/html", "chromium.desktop"));
    }

    #[test]
    fn is_pending_row_flags_pending_remove() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("chromium.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_remove_assoc("text/html", "chromium.desktop");
        assert!(app.is_pending_row("text/html", "chromium.desktop"));
        assert!(app.is_pending_removed_row("text/html", "chromium.desktop"));
    }

    #[test]
    fn displayable_assoc_keeps_pending_removed_rows() {
        let (apps, mimes, mut assoc) = sample_world();
        // Both apps declare text/html via .desktop, so both appear in the
        // effective list. After removing one, the effective list drops it
        // but the displayable list keeps it.
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);
        assert_eq!(app.effective_associations_for("text/html").len(), 2);

        app.action_remove_assoc("text/html", "chromium.desktop");
        assert_eq!(
            app.effective_associations_for("text/html").len(),
            1,
            "effective list drops the pending-removed row"
        );

        let displayable = app.displayable_associations_for("text/html");
        assert_eq!(
            displayable.len(),
            2,
            "displayable list keeps the pending-removed row visible"
        );
        let removed_flags: Vec<(String, bool)> = displayable
            .iter()
            .map(|(a, r)| (a.id.clone(), *r))
            .collect();
        assert!(removed_flags.contains(&("chromium.desktop".into(), true)));
        assert!(removed_flags.contains(&("firefox.desktop".into(), false)));
    }

    #[test]
    fn displayable_mime_list_for_app_keeps_pending_removed_rows() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        // Baseline: chromium has Associated relation for text/html (it
        // declares the mime in its .desktop).
        let baseline = app.displayable_mime_list_for_app("chromium.desktop");
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].1, Relation::Associated);
        assert!(!baseline[0].2, "no pending edits → not flagged removed");

        app.action_remove_assoc("text/html", "chromium.desktop");

        // Displayable view keeps the row with its *pre-remove* relation
        // (Associated, not the post-remove DeclaredOnly fallthrough), and
        // sets the pending-removed flag so the UI can strikethrough it.
        let after = app.displayable_mime_list_for_app("chromium.desktop");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].0.id, "text/html");
        assert_eq!(after[0].1, Relation::Associated);
        assert!(after[0].2, "expected pending-removed flag true");
    }

    #[test]
    fn removing_the_default_cascades_to_clear_it() {
        // firefox is the on-disk default for text/html. Removing it
        // should also clear the default so we don't leave a dangling
        // `[Default Applications]` entry pointing at a row that's
        // simultaneously in `[Removed Associations]`.
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        let cleared = app.action_remove_assoc("text/html", "firefox.desktop");
        assert!(cleared, "should signal that the default was also cleared");

        // pending.set_default reflects the cascade: Some(None) means
        // "explicitly clear the default on save".
        let slot = app
            .pending
            .set_default
            .get("text/html")
            .expect("default change should be staged");
        assert!(slot.is_none(), "expected Some(None), got {:?}", slot);

        // The displayable view drops the ★ on the strikethrough row —
        // it's no longer the default after this remove takes effect.
        let displayable = app.displayable_mime_list_for_app("firefox.desktop");
        let row = displayable
            .iter()
            .find(|(m, _, _)| m.id == "text/html")
            .expect("firefox should still appear in displayable list");
        assert!(row.2, "pending-removed flag should be true");
        assert_eq!(row.1, Relation::Associated);
    }

    #[test]
    fn removing_non_default_does_not_clear_default() {
        // chromium is associated but not the default — removing it
        // mustn't touch the default for the mime.
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        let cleared = app.action_remove_assoc("text/html", "chromium.desktop");
        assert!(!cleared);
        assert!(
            !app.pending.set_default.contains_key("text/html"),
            "default state must be untouched when removing a non-default row"
        );
    }

    #[test]
    fn removing_phantom_default_cascades_clear_too() {
        // Loupe is the on-disk default for image/jxl but isn't installed
        // — the phantom-default case the user surfaced. Removing it
        // should clear the default so saving doesn't leave an orphan
        // `[Default Applications]` line behind.
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("image/jxl".into(), "org.gnome.Loupe.desktop".into());
        let mut mimes = mimes;
        mimes.push(MimeType {
            id: "image/jxl".into(),
            description: "JPEG XL image".into(),
        });
        let mut app = App::for_test(apps, mimes, assoc);

        let cleared = app.action_remove_assoc("image/jxl", "org.gnome.Loupe.desktop");
        assert!(cleared);
        let slot = app.pending.set_default.get("image/jxl").unwrap();
        assert!(slot.is_none(), "phantom default should be cleared too");
    }

    #[test]
    fn remove_cancels_pending_add_for_off_disk_row() {
        // Build a world where firefox is the only declared handler for
        // text/html, so adding a third app means a pure pending.add row
        // with no on-disk anchor.
        let (mut apps, mimes, assoc) = sample_world();
        apps.push(DesktopApp {
            id: "elinks.desktop".into(),
            name: "ELinks".into(),
            comment: "".into(),
            exec: "elinks".into(),
            terminal: true,
            mime_types: vec![], // doesn't declare text/html
            category: "Network".into(),
        });
        let mut app = App::for_test(apps, mimes, assoc);

        // Add an off-disk app via the picker path.
        app.action_add_assoc("text/html", "elinks.desktop");
        assert!(app.is_dirty());
        assert_eq!(app.pending.count(), 1);

        // Pressing `r` on this newly-added row should fully undo the edit —
        // no pending.remove entry, no lingering pending.add.
        app.action_remove_assoc("text/html", "elinks.desktop");
        assert_eq!(
            app.pending.count(),
            0,
            "added-then-removed off-disk row should leave pending edits empty"
        );
        assert!(!app.is_dirty());
        // And it disappears from the displayable list (no on-disk anchor,
        // no pending edit to keep it visible).
        let displayable: Vec<&str> = app
            .displayable_associations_for("text/html")
            .iter()
            .map(|(a, _)| a.id.as_str())
            .collect();
        assert!(
            !displayable.contains(&"elinks.desktop"),
            "row should vanish, not stay as strikethrough"
        );
    }

    #[test]
    fn remove_cancels_pending_default_for_off_disk_row() {
        // User adds an off-disk app, sets it as default, then removes it.
        // The pending default should be cleaned up too — otherwise we'd
        // leave a dangling default pointing at an unassociated app.
        let (mut apps, mimes, assoc) = sample_world();
        apps.push(DesktopApp {
            id: "elinks.desktop".into(),
            name: "ELinks".into(),
            comment: "".into(),
            exec: "elinks".into(),
            terminal: true,
            mime_types: vec![],
            category: "Network".into(),
        });
        let mut app = App::for_test(apps, mimes, assoc);

        app.action_add_assoc("text/html", "elinks.desktop");
        app.action_set_default("text/html", "elinks.desktop");
        assert!(app.pending.set_default.contains_key("text/html"));

        app.action_remove_assoc("text/html", "elinks.desktop");
        assert!(
            !app.pending.set_default.contains_key("text/html"),
            "pending.set_default should be cleaned up when its target is cancelled"
        );
        assert_eq!(app.pending.count(), 0);
    }

    #[test]
    fn remove_of_on_disk_row_still_uses_pending_remove() {
        // Sanity: the off-disk shortcut must not change the behaviour of
        // removing a row that *does* have an on-disk anchor.
        let (apps, mimes, mut assoc) = sample_world();
        // chromium is declared via .desktop already; also put it in
        // [Added Associations] to make sure both paths are exercised.
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("chromium.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        app.action_remove_assoc("text/html", "chromium.desktop");
        assert!(app.is_pending_removed_row("text/html", "chromium.desktop"));
        assert_eq!(app.pending.count(), 1);
    }

    #[test]
    fn mime_has_missing_is_false_when_all_assocs_are_installed() {
        let (apps, mimes, mut assoc) = sample_world();
        // firefox is the default for text/html and it's installed.
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("chromium.desktop".into());
        let app = App::for_test(apps, mimes, assoc);
        assert!(!app.mime_has_missing("text/html"));
    }

    #[test]
    fn mime_has_missing_is_true_when_default_app_is_phantom() {
        // The Loupe case: default points at a non-installed .desktop.
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
        let mut mimes = mimes;
        mimes.push(MimeType {
            id: "image/png".into(),
            description: "PNG image".into(),
        });
        let app = App::for_test(apps, mimes, assoc);
        assert!(app.mime_has_missing("image/png"));
    }

    #[test]
    fn mime_has_missing_is_true_when_added_app_is_phantom() {
        // Default is fine, but [Added Associations] points at a phantom.
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("image/png".into(), "firefox.desktop".into());
        assoc
            .added
            .entry("image/png".into())
            .or_default()
            .insert("org.gnome.Loupe.desktop".into());
        let mut mimes = mimes;
        mimes.push(MimeType {
            id: "image/png".into(),
            description: "PNG image".into(),
        });
        let app = App::for_test(apps, mimes, assoc);
        assert!(app.mime_has_missing("image/png"));
    }

    #[test]
    fn missing_associations_surface_uninstalled_default_and_added() {
        // image/png has firefox as default (installed) AND a phantom
        // org.gnome.Loupe.desktop in added (not in the installed apps
        // index). image/jxl has only the phantom Loupe as the default.
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("image/png".into(), "firefox.desktop".into());
        assoc
            .added
            .entry("image/png".into())
            .or_default()
            .insert("org.gnome.Loupe.desktop".into());
        assoc
            .defaults
            .insert("image/jxl".into(), "org.gnome.Loupe.desktop".into());

        // Ensure the test world has the mimes too.
        let mut mimes = mimes;
        mimes.push(MimeType {
            id: "image/png".into(),
            description: "PNG image".into(),
        });
        mimes.push(MimeType {
            id: "image/jxl".into(),
            description: "JPEG XL image".into(),
        });

        let app = App::for_test(apps, mimes, assoc);

        // image/png: firefox is installed → not missing. Loupe is the
        // phantom → surfaces as missing, not the default for this mime.
        let png_missing = app.missing_associations_for("image/png");
        assert_eq!(png_missing.len(), 1);
        assert_eq!(png_missing[0].app_id, "org.gnome.Loupe.desktop");
        assert!(!png_missing[0].is_default);
        assert!(!png_missing[0].is_pending_removed);
        // Default for image/png resolves cleanly (firefox is installed).
        assert!(app.missing_default_for("image/png").is_none());

        // image/jxl: Loupe is the would-be default, but it's missing.
        let jxl_missing = app.missing_associations_for("image/jxl");
        assert_eq!(jxl_missing.len(), 1);
        assert_eq!(jxl_missing[0].app_id, "org.gnome.Loupe.desktop");
        assert!(jxl_missing[0].is_default);
        assert_eq!(
            app.missing_default_for("image/jxl").as_deref(),
            Some("org.gnome.Loupe.desktop"),
        );
    }

    #[test]
    fn missing_associations_reflect_pending_remove_of_phantom() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .added
            .entry("image/png".into())
            .or_default()
            .insert("org.gnome.Loupe.desktop".into());
        let mut mimes = mimes;
        mimes.push(MimeType {
            id: "image/png".into(),
            description: "PNG image".into(),
        });
        let mut app = App::for_test(apps, mimes, assoc);

        // User stages a cleanup of the phantom.
        app.action_remove_assoc("image/png", "org.gnome.Loupe.desktop");

        let missing = app.missing_associations_for("image/png");
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].app_id, "org.gnome.Loupe.desktop");
        assert!(missing[0].is_pending_removed);
    }

    #[test]
    fn missing_default_returns_none_when_pending_clears_it() {
        // Default on disk is the phantom, but user has staged a clear.
        // missing_default_for should return None (nothing to flag).
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("image/png".into(), "org.gnome.Loupe.desktop".into());
        let mut mimes = mimes;
        mimes.push(MimeType {
            id: "image/png".into(),
            description: "PNG image".into(),
        });
        let mut app = App::for_test(apps, mimes, assoc);

        app.action_clear_default("image/png");
        assert!(app.missing_default_for("image/png").is_none());
    }

    #[test]
    fn pending_change_summary_is_empty_when_clean() {
        let (apps, mimes, assoc) = sample_world();
        let app = App::for_test(apps, mimes, assoc);
        assert!(app.pending_change_summary().is_empty());
    }

    #[test]
    fn pending_change_summary_groups_per_mime_and_sorts() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("chromium.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        // Mixed edits across two mimes — verify grouping, sort order, and
        // that DefaultChange::Set carries the on-disk previous value.
        app.action_set_default("text/html", "chromium.desktop");
        app.action_remove_assoc("text/html", "firefox.desktop");
        app.apps[0].mime_types.clear();
        app.apps[1].mime_types.clear();
        app.action_add_assoc("application/pdf", "chromium.desktop");

        let summary = app.pending_change_summary();
        assert_eq!(summary.len(), 2);
        // Sorted by mime id: application/pdf before text/html.
        assert_eq!(summary[0].mime, "application/pdf");
        assert_eq!(summary[1].mime, "text/html");

        // application/pdf: just an add.
        assert_eq!(summary[0].default_change, None);
        assert_eq!(summary[0].adds, vec!["chromium.desktop"]);
        assert!(summary[0].removes.is_empty());

        // text/html: default change AND a remove. The default change must
        // carry the on-disk previous default in `old`.
        match &summary[1].default_change {
            Some(DefaultChange::Set { new, old }) => {
                assert_eq!(new, "chromium.desktop");
                assert_eq!(old.as_deref(), Some("firefox.desktop"));
            }
            other => panic!("expected Set default change, got {:?}", other),
        }
        assert_eq!(summary[1].removes, vec!["firefox.desktop"]);
    }

    #[test]
    fn pending_change_summary_captures_default_cleared() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);
        app.action_clear_default("text/html");

        let summary = app.pending_change_summary();
        assert_eq!(summary.len(), 1);
        match &summary[0].default_change {
            Some(DefaultChange::Cleared { old }) => {
                assert_eq!(old.as_deref(), Some("firefox.desktop"));
            }
            other => panic!("expected Cleared default change, got {:?}", other),
        }
    }

    #[test]
    fn pending_change_summary_sorts_adds_and_removes_within_mime() {
        let (apps, mimes, mut assoc) = sample_world();
        // Ensure both apps have an on-disk anchor so action_remove_assoc
        // writes pending.remove rather than cancelling an add.
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("chromium.desktop".into());
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        // Remove in non-alphabetical order — summary must sort them.
        app.action_remove_assoc("text/html", "firefox.desktop");
        app.action_remove_assoc("text/html", "chromium.desktop");

        let summary = app.pending_change_summary();
        assert_eq!(summary.len(), 1);
        assert_eq!(
            summary[0].removes,
            vec!["chromium.desktop", "firefox.desktop"]
        );
    }

    #[test]
    fn undo_remove_returns_row_to_clean_state() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .added
            .entry("text/html".into())
            .or_default()
            .insert("firefox.desktop".into());
        let mut app = App::for_test(apps, mimes, assoc);

        app.action_remove_assoc("text/html", "firefox.desktop");
        assert!(app.is_pending_removed_row("text/html", "firefox.desktop"));
        assert_eq!(app.pending.count(), 1);

        app.action_undo_remove("text/html", "firefox.desktop");

        // pending.remove is gone, and crucially pending.add is NOT populated
        // (which is what add_assoc would have done) — so the row is back to
        // its clean on-disk state with no net edit.
        assert!(!app.is_pending_removed_row("text/html", "firefox.desktop"));
        assert!(!app.is_pending_row("text/html", "firefox.desktop"));
        assert_eq!(app.pending.count(), 0);
        assert!(!app.is_dirty());
    }

    #[test]
    fn is_pending_row_false_for_clean_state() {
        let (apps, mimes, mut assoc) = sample_world();
        assoc
            .defaults
            .insert("text/html".into(), "firefox.desktop".into());
        let app = App::for_test(apps, mimes, assoc);
        assert!(!app.is_pending_row("text/html", "firefox.desktop"));
        assert!(!app.is_pending_row("text/html", "chromium.desktop"));
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
