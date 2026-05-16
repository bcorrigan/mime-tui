use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use eyre::Result;
use ratatui::layout::Rect;
use tui_input::InputRequest;
use tui_input::backend::crossterm::EventHandler;

use crate::app::{App, Focus, Mode, SaveResult, View};

/// Returns Ok(true) when the app should exit cleanly.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    // Ctrl-Z suspends regardless of mode, matching shell convention. The
    // actual teardown/resume dance happens in main.rs because that's where
    // the terminal handle lives; we just flag it here.
    if matches!(
        (key.code, key.modifiers),
        (KeyCode::Char('z'), KeyModifiers::CONTROL)
    ) {
        app.pending_suspend = true;
        return Ok(false);
    }

    match app.mode.clone() {
        Mode::Browse => handle_key_browse(app, key),
        Mode::PickApp { .. } | Mode::PickMime { .. } => handle_key_picker(app, key),
        Mode::ConfirmQuit => Ok(handle_key_confirm_quit(app, key)),
        Mode::Help => Ok(handle_key_help(app, key)),
        Mode::ConflictResolve { conflicts } => {
            Ok(handle_key_conflict_resolve(app, key, &conflicts))
        }
        Mode::ThemePick { selected, .. } => {
            Ok(handle_key_theme_pick(app, key, selected))
        }
        Mode::ConfirmSave => Ok(handle_key_confirm_save(app, key)),
    }
}

// ───────────── Browse ────────────────────────────────────────────────────────

fn handle_key_browse(app: &mut App, key: KeyEvent) -> Result<bool> {
    // Mode-changing globals first.
    match (key.code, key.modifiers) {
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            // Ctrl-S now routes through a review modal so the user gets
            // a last-chance summary of pending edits. Clean state still
            // flashes its no-op message inline.
            if app.is_dirty() {
                app.open_confirm_save();
            } else {
                app.set_flash("Nothing to save.");
            }
            return Ok(false);
        }
        (KeyCode::Char('t'), KeyModifiers::CONTROL) => {
            // Open the live theme preview.
            app.open_theme_pick();
            return Ok(false);
        }
        _ => {}
    }

    // Esc / Ctrl+C / Ctrl+G: from Focus::Right, just leave edit mode.
    // From elsewhere, try to quit (confirm first if dirty).
    let is_quit_keypress = matches!(
        (key.code, key.modifiers),
        (KeyCode::Esc, _)
            | (KeyCode::Char('c'), KeyModifiers::CONTROL)
            | (KeyCode::Char('g'), KeyModifiers::CONTROL)
    );
    if is_quit_keypress {
        if app.focus == Focus::Right {
            app.focus = Focus::Left;
            return Ok(false);
        }
        if app.is_dirty() {
            app.mode = Mode::ConfirmQuit;
            return Ok(false);
        }
        return Ok(true);
    }

    if key.code == KeyCode::Tab {
        app.toggle_view();
        app.focus = Focus::Left;
        return Ok(false);
    }

    // `?` opens help from any focus. Verified that no mime types or
    // installed .desktop app names contain '?', so this never shadows
    // legitimate search input.
    if key.code == KeyCode::Char('?') && key.modifiers.is_empty() {
        app.mode = Mode::Help;
        return Ok(false);
    }

    // Edit-mode actions. d/r/c operate on the currently-selected (mime, app)
    // pair when Focus::Right (where the second half of the pair is selected).
    // a opens the right picker for the current selection on the *left* — and
    // it's reachable from any focus.
    if key.modifiers.is_empty() {
        match (key.code, app.focus, app.view) {
            (KeyCode::Char('d'), Focus::Right, _) => {
                action_set_default(app);
                return Ok(false);
            }
            (KeyCode::Char('r'), Focus::Right, _) => {
                action_remove(app);
                return Ok(false);
            }
            (KeyCode::Char('c'), Focus::Right, _) => {
                action_clear_default(app);
                return Ok(false);
            }
            // 'a' is intentionally gated to Focus::Right so it doesn't shadow
            // search input when the user is filtering the left list. To reach
            // it: press → first (which works even with an empty right pane).
            (KeyCode::Char('a'), Focus::Right, View::ByMime) => {
                if let Some(m) = app.currently_selected_mime() {
                    let mime_id = m.id.clone();
                    app.open_pick_app(mime_id);
                }
                return Ok(false);
            }
            (KeyCode::Char('a'), Focus::Right, View::ByApp) => {
                if let Some(a) = app.currently_selected_app() {
                    let app_id = a.id.clone();
                    if app.is_phantom_app(&app_id) {
                        // Adding mime associations to a non-installed
                        // app would just plant fresh orphan entries —
                        // the opposite of cleanup.
                        app.set_flash(format!(
                            "Can't add associations — {} is not installed",
                            app_id
                        ));
                        return Ok(false);
                    }
                    app.open_pick_mime(app_id);
                }
                return Ok(false);
            }
            _ => {}
        }
    }

    // Shift+←/→ horizontally scrolls the focused list. Intercepted before
    // plain ←/→ which is reserved for pane navigation. Dispatches to
    // left_hscroll or right_hscroll based on which pane is focused.
    if key.modifiers == KeyModifiers::SHIFT {
        match (key.code, app.focus) {
            (KeyCode::Left, Focus::Right) => {
                app.right_hscroll = app.right_hscroll.saturating_sub(LEFT_HSCROLL_STEP);
                return Ok(false);
            }
            (KeyCode::Right, Focus::Right) => {
                app.right_hscroll = app.right_hscroll.saturating_add(LEFT_HSCROLL_STEP);
                return Ok(false);
            }
            (KeyCode::Left, _) => {
                app.left_hscroll = app.left_hscroll.saturating_sub(LEFT_HSCROLL_STEP);
                return Ok(false);
            }
            (KeyCode::Right, _) => {
                app.left_hscroll = app.left_hscroll.saturating_add(LEFT_HSCROLL_STEP);
                return Ok(false);
            }
            _ => {}
        }
    }

    // Emacs-style page nav: C-v down, M-v up. Intentionally undocumented in
    // the status bar — power-user affordance, not first-class UX.
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('v') {
        page_down(app);
        return Ok(false);
    }
    if key.modifiers == KeyModifiers::ALT
        && matches!(key.code, KeyCode::Char('v') | KeyCode::Char('V'))
    {
        page_up(app);
        return Ok(false);
    }
    // Emacs-style line nav: C-n / C-p as arrow aliases.
    if key.modifiers == KeyModifiers::CONTROL {
        match key.code {
            KeyCode::Char('n') => {
                navigate_down(app);
                return Ok(false);
            }
            KeyCode::Char('p') => {
                navigate_up(app);
                return Ok(false);
            }
            _ => {}
        }
    }

    // Navigation (all focuses).
    match key.code {
        KeyCode::PageDown => {
            page_down(app);
            return Ok(false);
        }
        KeyCode::PageUp => {
            page_up(app);
            return Ok(false);
        }
        KeyCode::Up => {
            navigate_up(app);
            return Ok(false);
        }
        KeyCode::Down => {
            navigate_down(app);
            return Ok(false);
        }
        KeyCode::Left => {
            if app.focus == Focus::Right {
                app.focus = Focus::Left;
            }
            return Ok(false);
        }
        KeyCode::Right | KeyCode::Enter => {
            // Right-arrow is the canonical "enter the right pane" key; we
            // also accept Enter because it's the natural reflex for "drill
            // in / accept", and the search input is live (Enter would do
            // nothing useful if forwarded there). On the right pane this
            // arm becomes a no-op — no Enter-bound action defined there.
            //
            // Always allow focusing the right pane — even when empty — so
            // the user has a reliable way to reach 'a' (add/picker) on
            // items that currently have no associations.
            if app.focus != Focus::Right {
                app.focus = Focus::Right;
                let count = right_pane_count(app);
                if count > 0 {
                    app.selected_right = app.selected_right.min(count - 1);
                } else {
                    app.selected_right = 0;
                }
            }
            return Ok(false);
        }
        _ => {}
    }

    // From Focus::Right we don't forward letter keys to search — that would
    // be surprising in edit context.
    if app.focus == Focus::Right {
        return Ok(false);
    }

    forward_to_input(&mut app.input, key);
    app.reset_cursor_blink();
    reset_left_selection(app);
    Ok(false)
}

fn navigate_up(app: &mut App) {
    if app.focus == Focus::Right {
        if app.selected_right > 0 {
            app.selected_right -= 1;
        }
        return;
    }
    if app.selected_left > 0 {
        app.selected_left -= 1;
    }
}

/// Best-effort page size derived from the most recent list height we drew.
/// Falls back to 10 when the rect hasn't been recorded yet.
fn browse_page_size(app: &App) -> usize {
    let rect = if app.focus == Focus::Right {
        app.right_rect
    } else {
        app.left_rect
    };
    rect.map(|r| (r.height as usize).saturating_sub(2).max(1))
        .unwrap_or(10)
}

fn page_up(app: &mut App) {
    let n = browse_page_size(app);
    if app.focus == Focus::Right {
        app.selected_right = app.selected_right.saturating_sub(n);
    } else {
        app.selected_left = app.selected_left.saturating_sub(n);
    }
}

fn page_down(app: &mut App) {
    let n = browse_page_size(app);
    if app.focus == Focus::Right {
        let count = right_pane_count(app);
        if count > 0 {
            app.selected_right = (app.selected_right + n).min(count - 1);
        }
    } else {
        let count = left_pane_count(app);
        if count > 0 {
            app.selected_left = (app.selected_left + n).min(count - 1);
        }
    }
}

fn navigate_down(app: &mut App) {
    if app.focus == Focus::Right {
        let count = right_pane_count(app);
        if count > 0 && app.selected_right + 1 < count {
            app.selected_right += 1;
        }
        return;
    }
    let count = left_pane_count(app);
    if count > 0 && app.selected_left + 1 < count {
        app.selected_left += 1;
    }
}

fn left_pane_count(app: &App) -> usize {
    match app.view {
        View::ByMime => app.visible_mimes().len(),
        View::ByApp => app.visible_apps().len(),
    }
}

fn right_pane_count(app: &App) -> usize {
    // The right pane renders the *displayable* lists (which keep
    // pending-removed rows visible-with-strikethrough). Indexing/clamping
    // must match — otherwise `selected_right` drifts relative to the rows
    // the user actually sees, which is how repeated `r` ended up removing
    // unrelated items further down the list.
    //
    // In by-mime we also append "missing" rows (phantom associations
    // pointing at uninstalled apps), so they're counted here too.
    match app.view {
        View::ByMime => {
            let Some(m) = app.currently_selected_mime() else {
                return 0;
            };
            let mime_id = m.id.clone();
            app.displayable_associations_for(&mime_id).len()
                + app.missing_associations_for(&mime_id).len()
        }
        View::ByApp => {
            let Some(a) = app.currently_selected_app() else {
                return 0;
            };
            app.displayable_mime_list_for_app(&a.id).len()
        }
    }
}

/// Columns to scroll on each Shift+←/→ press in by-mime / by-app left list.
const LEFT_HSCROLL_STEP: u16 = 8;

fn reset_left_selection(app: &mut App) {
    app.selected_left = 0;
    app.selected_right = 0;
    app.left_hscroll = 0;
    app.right_hscroll = 0;
}

/// Extract the (mime, app_id, app_name) currently targeted by the right pane
/// selection. View-aware on the *where it comes from* axis, view-agnostic on
/// the *what it returns* axis — the third element is always the app's display
/// name, so flash-message templates can be written once.
fn target_pair(app: &App) -> Option<(String, String, String)> {
    // Index into the *displayable* lists so the (mime, app) we resolve
    // matches the row the user has highlighted — including pending-removed
    // rows that the UI keeps visible with strikethrough.
    //
    // In by-mime, missing rows (uninstalled apps) are appended after the
    // installed/pending-removed rows; when the cursor sits on one of
    // those, the resolved name falls back to the app id since we don't
    // have a real .desktop name to show in flash messages.
    match app.view {
        View::ByMime => {
            let mime = app.currently_selected_mime()?;
            let mime_id = mime.id.clone();
            let assoc = app.displayable_associations_for(&mime_id);
            if let Some((entry, _is_removed)) = assoc.get(app.selected_right) {
                return Some((mime_id, entry.id.clone(), entry.name.clone()));
            }
            let missing = app.missing_associations_for(&mime_id);
            let m = missing.get(app.selected_right - assoc.len())?;
            Some((mime_id, m.app_id.clone(), m.app_id.clone()))
        }
        View::ByApp => {
            let a = app.currently_selected_app()?;
            let app_id = a.id.clone();
            let app_name = a.name.clone();
            let list = app.displayable_mime_list_for_app(&app_id);
            let (mime, _rel, _is_removed) = list.get(app.selected_right)?;
            Some((mime.id.clone(), app_id, app_name))
        }
    }
}

fn action_set_default(app: &mut App) {
    let Some((mime_id, app_id, app_name)) = target_pair(app) else {
        return;
    };
    if app.is_phantom_app(&app_id) {
        // Setting a non-installed app as default would just plant a
        // fresh orphan `[Default Applications]` entry — the very kind
        // of garbage we surface as red and offer to clean up.
        app.set_flash(format!(
            "Can't set default — {} is not installed",
            app_id
        ));
        return;
    }
    app.action_set_default(&mime_id, &app_id);
    app.set_flash(format!(
        "{} is now the default app for {}",
        app_name, mime_id
    ));
}

fn action_remove(app: &mut App) {
    let Some((mime_id, app_id, app_name)) = target_pair(app) else {
        return;
    };
    // Toggle: pressing `r` on an already-pending-removed row un-removes it
    // rather than re-asserting the (idempotent) remove. This makes `r` the
    // natural inverse of itself and avoids the previous footgun where
    // repeated taps walked down the effective list.
    if app.is_pending_removed_row(&mime_id, &app_id) {
        app.action_undo_remove(&mime_id, &app_id);
        app.set_flash(format!(
            "{} restored — no longer pending removal from {}",
            app_name, mime_id
        ));
        // Undo branch: user is correcting, not progressing through a list.
        // Keep the cursor on the row they just restored. Still clamp in
        // case some other state has shifted the list bounds.
        let new_count = right_pane_count(app);
        if new_count == 0 {
            app.focus = Focus::Left;
            app.selected_right = 0;
        } else if app.selected_right >= new_count {
            app.selected_right = new_count - 1;
        }
        return;
    }

    // Capture the row's stable identifier *before* the mutation so we can
    // find its new index in the post-mutation list and advance the cursor
    // past it. In by-app the row is keyed by mime id; in by-mime by app id.
    let anchor = match app.view {
        View::ByApp => mime_id.clone(),
        View::ByMime => app_id.clone(),
    };

    let cleared_default = app.action_remove_assoc(&mime_id, &app_id);
    let msg = if cleared_default {
        format!(
            "{} removed from {} (also cleared as default)",
            app_name, mime_id
        )
    } else {
        format!("{} is no longer associated with {}", app_name, mime_id)
    };
    app.set_flash(msg);

    let new_count = right_pane_count(app);
    if new_count == 0 {
        app.focus = Focus::Left;
        app.selected_right = 0;
        return;
    }

    match find_right_row_index(app, &anchor) {
        Some(idx) => {
            // Row still visible (strikethrough remove). Advance past it so
            // repeated `r`s walk down the list.
            app.selected_right = (idx + 1).min(new_count - 1);
        }
        None => {
            // Row vanished — the pending-add cancel path. The row that was
            // below has shifted up into this slot, so the current
            // `selected_right` already points there; just clamp.
            app.selected_right = app.selected_right.min(new_count - 1);
        }
    }
}

/// Locate `anchor` in the current right-pane list and return its index, or
/// `None` if it's no longer present. View-aware: in by-app the anchor is a
/// mime id (rows keyed by mime); in by-mime it's an app id (rows keyed by
/// app, with phantom missing-assoc rows appended after the installed list).
fn find_right_row_index(app: &App, anchor: &str) -> Option<usize> {
    match app.view {
        View::ByApp => {
            let selected = app.currently_selected_app()?;
            let list = app.displayable_mime_list_for_app(&selected.id);
            list.iter().position(|(m, _, _)| m.id == anchor)
        }
        View::ByMime => {
            let selected = app.currently_selected_mime()?;
            let mime_id = selected.id.clone();
            let assoc = app.displayable_associations_for(&mime_id);
            if let Some(i) = assoc.iter().position(|(a, _)| a.id == anchor) {
                return Some(i);
            }
            // Phantoms occupy the tail of the right list — same coordinate
            // system as `target_pair`'s by-mime branch.
            let missing = app.missing_associations_for(&mime_id);
            missing
                .iter()
                .position(|m| m.app_id == anchor)
                .map(|i| i + assoc.len())
        }
    }
}

fn action_clear_default(app: &mut App) {
    let Some((mime_id, _, _)) = target_pair(app) else {
        return;
    };
    app.action_clear_default(&mime_id);
    app.set_flash(format!("{} now has no default app", mime_id));
}

/// Columns to scroll horizontally on Shift+←/→ in the confirm-save modal.
const CONFIRM_SAVE_HSCROLL_STEP: u16 = 8;
/// Page size for PgUp/PgDn/Space/b. A modest value works regardless of the
/// actual modal height — repeated presses page through cleanly.
const CONFIRM_SAVE_PAGE: u16 = 10;

fn handle_key_confirm_save(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        // Accept — commit the pending edits.
        (KeyCode::Enter, _)
        | (KeyCode::Char('y'), _)
        | (KeyCode::Char('Y'), _) => {
            // Don't go through close_confirm_save here: it would clear
            // quit_after_save before we've decided whether to honour it.
            let should_quit = app.quit_after_save;
            app.mode = Mode::Browse;
            do_save(app);
            // Clean save → we're back in Browse with nothing pending. If the
            // user got here via "save & quit", exit. Otherwise drop the
            // intent so a future Ctrl-S doesn't surprise-quit.
            // A conflict reroutes to Mode::ConflictResolve; keep the flag
            // alive so the eventual resolution can quit too.
            if matches!(app.mode, Mode::Browse) && !app.is_dirty() && should_quit {
                return true;
            }
            if !matches!(app.mode, Mode::ConflictResolve { .. }) {
                app.quit_after_save = false;
            }
        }
        // Cancel — back to Browse with pending edits intact.
        (KeyCode::Esc, _)
        | (KeyCode::Char('n'), _)
        | (KeyCode::Char('N'), _)
        | (KeyCode::Char('q'), _)
        | (KeyCode::Char('Q'), _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
            app.close_confirm_save();
        }
        // Vertical scroll. The renderer clamps `confirm_save_vscroll`
        // against the actual content height every frame, so we don't need
        // exact bounds here.
        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            app.confirm_save_vscroll = app.confirm_save_vscroll.saturating_sub(1);
        }
        (KeyCode::Down, _) | (KeyCode::Char('j'), KeyModifiers::NONE) => {
            app.confirm_save_vscroll = app.confirm_save_vscroll.saturating_add(1);
        }
        (KeyCode::PageUp, _) | (KeyCode::Char('b'), KeyModifiers::NONE) => {
            app.confirm_save_vscroll =
                app.confirm_save_vscroll.saturating_sub(CONFIRM_SAVE_PAGE);
        }
        (KeyCode::PageDown, _) | (KeyCode::Char(' '), KeyModifiers::NONE) => {
            app.confirm_save_vscroll =
                app.confirm_save_vscroll.saturating_add(CONFIRM_SAVE_PAGE);
        }
        (KeyCode::Home, _) | (KeyCode::Char('g'), KeyModifiers::NONE) => {
            app.confirm_save_vscroll = 0;
        }
        (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
            // Renderer will clamp this against the actual content height.
            app.confirm_save_vscroll = u16::MAX;
        }
        // Horizontal scroll for long mime ids that overflow the modal.
        (KeyCode::Left, KeyModifiers::SHIFT)
        | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            app.confirm_save_hscroll = app
                .confirm_save_hscroll
                .saturating_sub(CONFIRM_SAVE_HSCROLL_STEP);
        }
        (KeyCode::Right, KeyModifiers::SHIFT)
        | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            app.confirm_save_hscroll = app
                .confirm_save_hscroll
                .saturating_add(CONFIRM_SAVE_HSCROLL_STEP);
        }
        _ => {}
    }
    false
}

fn do_save(app: &mut App) {
    if !app.is_dirty() {
        app.set_flash("Nothing to save.");
        return;
    }
    match app.save() {
        Ok(SaveResult::Saved { written, shadowed, merged_external }) => {
            let mut msg = format!("Saved {} edit(s) to mimeapps.list", written);
            if merged_external {
                msg.push_str(" — merged with concurrent external changes");
            }
            if !shadowed.is_empty() {
                msg.push_str(&format!(
                    " — {} default{} shadowed by a per-desktop override (see detail pane)",
                    shadowed.len(),
                    if shadowed.len() == 1 { "" } else { "s" },
                ));
            }
            app.set_flash(msg);
        }
        Ok(SaveResult::Conflicts(conflicts)) => {
            app.set_flash(format!(
                "mimeapps.list changed externally — {} conflict{} (see modal)",
                conflicts.len(),
                if conflicts.len() == 1 { "" } else { "s" },
            ));
            app.mode = Mode::ConflictResolve { conflicts };
        }
        Err(e) => app.set_flash(format!("Save failed: {}", e)),
    }
}

// ───────────── Picker ────────────────────────────────────────────────────────

/// Fixed page size for picker nav. The picker's list height isn't tracked
/// (we'd need a `pick_list_rect: Option<Rect>` plumbed through draw), and a
/// generic chunk works fine in practice since users can page repeatedly.
const PICKER_PAGE: usize = 10;

/// Columns to scroll on each ←/→/Ctrl-B/Ctrl-F press in the picker.
const PICKER_HSCROLL_STEP: u16 = 8;


fn handle_key_picker(app: &mut App, key: KeyEvent) -> Result<bool> {
    match (key.code, key.modifiers) {
        // Always close the picker (Ctrl+C and Ctrl+G keep their Browse-mode
        // "cancel" meaning here too).
        (KeyCode::Esc, _)
        | (KeyCode::Char('c'), KeyModifiers::CONTROL)
        | (KeyCode::Char('g'), KeyModifiers::CONTROL) => {
            app.close_picker();
            return Ok(false);
        }

        // Mark / unmark for emacs-style range selection. Different terminals
        // encode Ctrl+Space as Char(' ')+CONTROL, Char('@')+CONTROL, or
        // KeyCode::Null — accept all three so the binding works everywhere.
        (KeyCode::Char(' '), KeyModifiers::CONTROL)
        | (KeyCode::Char('@'), KeyModifiers::CONTROL)
        | (KeyCode::Null, _) => {
            app.picker_toggle_mark();
            return Ok(false);
        }

        // Space toggles the cursor row (or the marked range) and stays in
        // the picker — meant for rapid multi-entry editing.
        (KeyCode::Char(' '), KeyModifiers::NONE) => {
            app.picker_apply_toggle();
            return Ok(false);
        }

        // Enter is the "accept" key — toggle, then dismiss the modal. The
        // user has signalled "I'm done picking", so we hand them back to
        // the main view instead of waiting for an Esc.
        (KeyCode::Enter, _) => {
            app.picker_apply_toggle();
            app.close_picker();
            return Ok(false);
        }

        (KeyCode::Up, _) | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            if app.pick_selected > 0 {
                app.pick_selected -= 1;
            }
            return Ok(false);
        }
        (KeyCode::Down, _) | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            let count = picker_visible_count(app);
            if count > 0 && app.pick_selected + 1 < count {
                app.pick_selected += 1;
            }
            return Ok(false);
        }
        (KeyCode::PageUp, _) | (KeyCode::Char('v'), KeyModifiers::ALT)
        | (KeyCode::Char('V'), KeyModifiers::ALT) => {
            app.pick_selected = app.pick_selected.saturating_sub(PICKER_PAGE);
            return Ok(false);
        }
        (KeyCode::PageDown, _) | (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
            let count = picker_visible_count(app);
            if count > 0 {
                app.pick_selected = (app.pick_selected + PICKER_PAGE).min(count - 1);
            }
            return Ok(false);
        }

        // Horizontal scroll for long mime ids / descriptions that overflow
        // the picker width. Intercepted before forward_to_input so Ctrl-F /
        // Ctrl-B don't fall through to char-by-char input nav (Ctrl-A /
        // Ctrl-E still cover start/end of the search input).
        (KeyCode::Left, _) | (KeyCode::Char('b'), KeyModifiers::CONTROL) => {
            app.pick_hscroll = app.pick_hscroll.saturating_sub(PICKER_HSCROLL_STEP);
            return Ok(false);
        }
        (KeyCode::Right, _) | (KeyCode::Char('f'), KeyModifiers::CONTROL) => {
            app.pick_hscroll = app.pick_hscroll.saturating_add(PICKER_HSCROLL_STEP);
            return Ok(false);
        }

        _ => {}
    }
    forward_to_input(&mut app.pick_input, key);
    // Filter changed → the previous mark refers to a different visual row.
    // Easier to clear than to track which mime/app it had pointed at.
    app.pick_selected = 0;
    app.pick_mark = None;
    app.pick_hscroll = 0;
    app.reset_cursor_blink();
    Ok(false)
}

/// Mode-aware "how many candidates does the picker currently show" — gates
/// the Down-arrow at the bottom of the list. Without this, a mime-picker
/// gets clamped to the app-list count (which has nothing to do with the
/// user's query).
fn picker_visible_count(app: &App) -> usize {
    match app.mode {
        Mode::PickApp { .. } => app.picker_visible_apps().len(),
        Mode::PickMime { .. } => app.picker_visible_mimes().len(),
        _ => 0,
    }
}

// ───────────── Help ──────────────────────────────────────────────────────────

const HELP_PAGE: u16 = 10;

fn handle_key_help(app: &mut App, key: KeyEvent) -> bool {
    match (key.code, key.modifiers) {
        // Explicit close. Reset scroll so next open starts at the top.
        (KeyCode::Esc, _)
        | (KeyCode::Enter, _)
        | (KeyCode::Char('q'), _)
        | (KeyCode::Char('Q'), _) => {
            app.mode = Mode::Browse;
            app.help_scroll = 0;
        }

        // Page down: Space, PgDn, Ctrl-V.
        (KeyCode::Char(' '), _)
        | (KeyCode::PageDown, _)
        | (KeyCode::Char('v'), KeyModifiers::CONTROL) => {
            app.help_scroll = app.help_scroll.saturating_add(HELP_PAGE);
        }
        // Page up: b, PgUp, Alt-V.
        (KeyCode::Char('b'), _)
        | (KeyCode::PageUp, _)
        | (KeyCode::Char('v'), KeyModifiers::ALT)
        | (KeyCode::Char('V'), KeyModifiers::ALT) => {
            app.help_scroll = app.help_scroll.saturating_sub(HELP_PAGE);
        }

        // Single-line nav (arrows + vim/emacs aliases).
        (KeyCode::Down, _)
        | (KeyCode::Char('j'), _)
        | (KeyCode::Char('n'), KeyModifiers::CONTROL) => {
            app.help_scroll = app.help_scroll.saturating_add(1);
        }
        (KeyCode::Up, _)
        | (KeyCode::Char('k'), _)
        | (KeyCode::Char('p'), KeyModifiers::CONTROL) => {
            app.help_scroll = app.help_scroll.saturating_sub(1);
        }

        // Top / bottom.
        (KeyCode::Home, _) | (KeyCode::Char('g'), _) => {
            app.help_scroll = 0;
        }
        (KeyCode::End, _) | (KeyCode::Char('G'), _) => {
            // u16::MAX → clamped to max_scroll by the renderer.
            app.help_scroll = u16::MAX;
        }

        _ => {
            // Unknown keys are no-ops so the user can scroll without
            // accidentally dismissing the help.
        }
    }
    false
}

// ───────────── ConflictResolve ───────────────────────────────────────────────

fn handle_key_conflict_resolve(
    app: &mut App,
    key: KeyEvent,
    conflicts: &[crate::storage::mimeapps::MimeConflict],
) -> bool {
    match key.code {
        KeyCode::Char('r') | KeyCode::Char('R') => {
            // Reload: discard pending, take the on-disk state as gospel.
            app.reload_from_disk();
            app.mode = Mode::Browse;
            // Reload means the user stepped out of the "save & quit" flow:
            // they're now exploring on-disk state, not exiting.
            app.quit_after_save = false;
            app.set_flash("Discarded pending edits; reloaded from disk.");
            false
        }
        KeyCode::Char('o') | KeyCode::Char('O') => {
            // Overwrite: clobber external changes.
            let should_quit = app.quit_after_save;
            match app.save_force() {
                Ok((n, _shadowed)) => {
                    app.mode = Mode::Browse;
                    app.quit_after_save = false;
                    app.set_flash(format!(
                        "Force-saved {} edit(s); external changes were overwritten.",
                        n
                    ));
                    if should_quit {
                        return true;
                    }
                }
                Err(e) => app.set_flash(format!("Force save failed: {}", e)),
            }
            false
        }
        KeyCode::Char('m') | KeyCode::Char('M') => {
            // Merge: drop the conflicting pending edits, save the rest.
            let dropped = conflicts.len();
            let should_quit = app.quit_after_save;
            match app.save_dropping_conflicts(conflicts) {
                Ok(SaveResult::Saved { written, .. }) => {
                    app.mode = Mode::Browse;
                    app.quit_after_save = false;
                    app.set_flash(format!(
                        "Saved {} edit(s); dropped {} conflicting one{}.",
                        written,
                        dropped,
                        if dropped == 1 { "" } else { "s" },
                    ));
                    if should_quit {
                        return true;
                    }
                }
                Ok(SaveResult::Conflicts(remaining)) => {
                    // Shouldn't happen — drop_conflicting_edits cleared them
                    // — but route to a fresh ConflictResolve just in case.
                    app.mode = Mode::ConflictResolve { conflicts: remaining };
                }
                Err(e) => app.set_flash(format!("Merge save failed: {}", e)),
            }
            false
        }
        KeyCode::Esc | KeyCode::Char('c') | KeyCode::Char('C') => {
            // Cancel: keep pending in memory, dismiss the modal. Backing out
            // of conflict resolution also abandons the quit intent.
            app.mode = Mode::Browse;
            app.quit_after_save = false;
            false
        }
        _ => false,
    }
}

// ───────────── ThemePick ─────────────────────────────────────────────────────

fn handle_key_theme_pick(app: &mut App, key: KeyEvent, selected: usize) -> bool {
    let count = crate::config::PRESET_NAMES.len();
    match key.code {
        KeyCode::Up | KeyCode::Char('k') => {
            if selected > 0 {
                app.preview_theme(selected - 1);
            }
            false
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if selected + 1 < count {
                app.preview_theme(selected + 1);
            }
            false
        }
        KeyCode::Home | KeyCode::Char('g') => {
            app.preview_theme(0);
            false
        }
        KeyCode::End | KeyCode::Char('G') => {
            if count > 0 {
                app.preview_theme(count - 1);
            }
            false
        }
        KeyCode::Enter => {
            // Commit the preview — keep the new theme and persist the
            // preset name into the user's mime-tui.toml so the choice
            // survives the next launch. Falls back to a session-only flash
            // if the disk write fails (read-only home, missing XDG dirs,
            // etc.).
            app.close_theme_pick(false);
            let name = app
                .active_preset
                .clone()
                .unwrap_or_else(|| "default-dark".into());
            match crate::config::save_preset_to_config(&name) {
                Ok(()) => app.set_flash(format!(
                    "Saved theme '{}' to ~/.config/mime-tui/mime-tui.toml",
                    name
                )),
                Err(e) => app.set_flash(format!(
                    "Theme kept for this session — couldn't update config: {}",
                    e
                )),
            }
            false
        }
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.close_theme_pick(true);
            false
        }
        _ => false,
    }
}

// ───────────── ConfirmQuit ───────────────────────────────────────────────────

fn handle_key_confirm_quit(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.action_discard();
            true
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            // Route through the same review modal Ctrl-S uses. The flag tells
            // the confirm-save handler to exit on a clean save instead of
            // returning to Browse.
            app.quit_after_save = true;
            app.open_confirm_save();
            false
        }
        _ => {
            // Anything else cancels and returns to Browse.
            app.mode = Mode::Browse;
            false
        }
    }
}

// ───────────── Input forwarding (shared by main search + picker) ─────────────

fn forward_to_input(input: &mut tui_input::Input, key: KeyEvent) {
    // Emacs-style readline bindings.
    let req = match key {
        KeyEvent { code: KeyCode::Char('a'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::GoToStart),
        KeyEvent { code: KeyCode::Char('e'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::GoToEnd),
        KeyEvent { code: KeyCode::Char('b'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::GoToPrevChar),
        KeyEvent { code: KeyCode::Char('f'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::GoToNextChar),
        KeyEvent { code: KeyCode::Char('w'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::DeletePrevWord),
        KeyEvent { code: KeyCode::Char('d'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::DeleteNextChar),
        KeyEvent { code: KeyCode::Char('h'), modifiers: KeyModifiers::CONTROL, .. } => Some(InputRequest::DeletePrevChar),
        _ => None,
    };

    if key.modifiers == KeyModifiers::CONTROL {
        match key.code {
            KeyCode::Char('u') => {
                let cursor = input.cursor();
                let val = input.value();
                if cursor > 0 && cursor <= val.len() {
                    let suffix = &val[cursor..];
                    let mut new_input = tui_input::Input::new(suffix.to_string());
                    new_input.handle(InputRequest::GoToStart);
                    *input = new_input;
                }
                return;
            }
            KeyCode::Char('k') => {
                let cursor = input.cursor();
                let val = input.value();
                if cursor < val.len() {
                    let prefix = &val[..cursor];
                    *input = tui_input::Input::new(prefix.to_string());
                }
                return;
            }
            _ => {}
        }
    }

    if let Some(req) = req {
        input.handle(req);
    } else {
        input.handle_event(&Event::Key(key));
    }
}

// ───────────── Mouse ────────────────────────────────────────────────────────

const WHEEL_STEP: usize = 3;

pub fn handle_mouse(app: &mut App, ev: MouseEvent) -> Result<bool> {
    // Status-bar clicks fire the represented keybinding before mode-specific
    // dispatch — each hint chunk carries the KeyEvent it stands for, so
    // clicking it is equivalent to typing that key. Wraps and re-uses every
    // existing key-dispatch path; no per-mode click logic needed.
    if let MouseEventKind::Down(MouseButton::Left) = ev.kind {
        let hit = app
            .status_clickables
            .iter()
            .find(|(rect, _)| rect_contains(*rect, ev.column, ev.row))
            .map(|(_, key)| *key);
        if let Some(key) = hit {
            return handle_key(app, key);
        }
    }

    match app.mode.clone() {
        Mode::Browse => handle_mouse_browse(app, ev),
        Mode::PickApp { .. } | Mode::PickMime { .. } => handle_mouse_picker(app, ev),
        Mode::Help => Ok(handle_mouse_help(app, ev)),
        Mode::ThemePick { selected, .. } => {
            Ok(handle_mouse_theme_pick(app, ev, selected))
        }
        Mode::ConfirmSave => Ok(handle_mouse_confirm_save(app, ev)),
        // These modals are small and decision-focused — keyboard-only
        // for the body, but their status-bar hints (above) are still
        // clickable via the dispatcher prelude.
        Mode::ConfirmQuit | Mode::ConflictResolve { .. } => Ok(false),
    }
}

fn handle_mouse_confirm_save(app: &mut App, ev: MouseEvent) -> bool {
    // The modal owns most of the screen; route any wheel event to vertical
    // scroll without bothering with hit-tests, matching the help overlay's
    // pragmatic stance.
    match ev.kind {
        MouseEventKind::ScrollUp => {
            app.confirm_save_vscroll = app
                .confirm_save_vscroll
                .saturating_sub(WHEEL_STEP as u16);
        }
        MouseEventKind::ScrollDown => {
            app.confirm_save_vscroll = app
                .confirm_save_vscroll
                .saturating_add(WHEEL_STEP as u16);
        }
        _ => {}
    }
    false
}

fn handle_mouse_theme_pick(app: &mut App, ev: MouseEvent, selected: usize) -> bool {
    let count = crate::config::PRESET_NAMES.len();
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // Click on a list row → preview that preset directly. The
            // theme list lives inside the modal's already-bordered inner
            // area (no per-list border), so we map row directly to index
            // without `click_row_in_list`'s 1-cell border offset.
            if let Some(rect) = app.theme_list_rect {
                if rect_contains(rect, ev.column, ev.row) {
                    let target = (ev.row - rect.y) as usize;
                    if target < count {
                        app.preview_theme(target);
                    }
                }
            }
        }
        MouseEventKind::ScrollUp => {
            if selected > 0 {
                app.preview_theme(selected - 1);
            }
        }
        MouseEventKind::ScrollDown => {
            if selected + 1 < count {
                app.preview_theme(selected + 1);
            }
        }
        _ => {}
    }
    false
}

fn handle_mouse_browse(app: &mut App, ev: MouseEvent) -> Result<bool> {
    let (col, row) = (ev.column, ev.row);

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if let Some(rect) = app.left_rect {
                if rect_contains(rect, col, row) {
                    if let Some(target) = click_row_in_list(rect, col, row) {
                        let offset = app.left_list_state.offset();
                        let display_idx = offset + target;
                        if display_idx < left_pane_count(app) {
                            app.focus = Focus::Left;
                            app.selected_left = display_idx;
                            app.selected_right = 0;
                        }
                    }
                    return Ok(false);
                }
            }
            if let Some(rect) = app.right_rect {
                if rect_contains(rect, col, row) {
                    if let Some(target) = click_row_in_list(rect, col, row) {
                        let offset = app.right_list_state.offset();
                        let display_idx = offset + target;
                        if display_idx < right_pane_count(app) {
                            app.focus = Focus::Right;
                            app.selected_right = display_idx;
                        }
                    }
                    return Ok(false);
                }
            }
            if let Some(rect) = app.search_rect {
                if rect_contains(rect, col, row) {
                    app.focus = Focus::Search;
                    return Ok(false);
                }
            }
        }
        MouseEventKind::ScrollUp => {
            // Scroll wherever the cursor is. Right pane scroll moves
            // selected_right; everywhere else (including search bar) moves
            // selected_left, since the left list is the primary surface.
            if let Some(rect) = app.right_rect {
                if rect_contains(rect, col, row) {
                    scroll_right_pane(app, -(WHEEL_STEP as isize));
                    return Ok(false);
                }
            }
            scroll_left_pane(app, -(WHEEL_STEP as isize));
            return Ok(false);
        }
        MouseEventKind::ScrollDown => {
            if let Some(rect) = app.right_rect {
                if rect_contains(rect, col, row) {
                    scroll_right_pane(app, WHEEL_STEP as isize);
                    return Ok(false);
                }
            }
            scroll_left_pane(app, WHEEL_STEP as isize);
            return Ok(false);
        }
        _ => {}
    }
    Ok(false)
}

fn handle_mouse_picker(app: &mut App, ev: MouseEvent) -> Result<bool> {
    let (col, row) = (ev.column, ev.row);
    let Some(rect) = app.pick_list_rect else {
        return Ok(false);
    };

    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            if rect_contains(rect, col, row) {
                if let Some(target) = click_row_in_list(rect, col, row) {
                    let offset = app.pick_list_state.offset();
                    let display_idx = offset + target;
                    if display_idx < picker_visible_count(app) {
                        app.pick_selected = display_idx;
                    }
                }
            }
            Ok(false)
        }
        MouseEventKind::ScrollUp => {
            if rect_contains(rect, col, row) {
                app.pick_selected = app.pick_selected.saturating_sub(WHEEL_STEP);
            }
            Ok(false)
        }
        MouseEventKind::ScrollDown => {
            if rect_contains(rect, col, row) {
                let count = picker_visible_count(app);
                if count > 0 {
                    app.pick_selected = (app.pick_selected + WHEEL_STEP).min(count - 1);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

fn handle_mouse_help(app: &mut App, ev: MouseEvent) -> bool {
    // Don't bother hit-testing inside the overlay — the help fills most of
    // the screen and any scroll anywhere should drive the overlay (no other
    // surface is interactive while help is up).
    match ev.kind {
        MouseEventKind::ScrollUp => {
            app.help_scroll = app.help_scroll.saturating_sub(WHEEL_STEP as u16);
        }
        MouseEventKind::ScrollDown => {
            app.help_scroll = app.help_scroll.saturating_add(WHEEL_STEP as u16);
        }
        _ => {}
    }
    false
}

fn scroll_left_pane(app: &mut App, delta: isize) {
    if delta < 0 {
        app.selected_left = app.selected_left.saturating_sub((-delta) as usize);
        return;
    }
    let count = left_pane_count(app);
    if count == 0 {
        return;
    }
    app.selected_left = (app.selected_left + delta as usize).min(count - 1);
}

fn scroll_right_pane(app: &mut App, delta: isize) {
    if delta < 0 {
        app.selected_right = app.selected_right.saturating_sub((-delta) as usize);
        return;
    }
    let count = right_pane_count(app);
    if count == 0 {
        return;
    }
    app.selected_right = (app.selected_right + delta as usize).min(count - 1);
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

fn click_row_in_list(rect: Rect, col: u16, row: u16) -> Option<usize> {
    if col < rect.x || col >= rect.x + rect.width {
        return None;
    }
    if row <= rect.y || row + 1 >= rect.y + rect.height {
        return None;
    }
    Some((row - rect.y - 1) as usize)
}
