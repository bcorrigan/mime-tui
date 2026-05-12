use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use eyre::Result;
use ratatui::layout::Rect;
use tui_input::InputRequest;
use tui_input::backend::crossterm::EventHandler;

use crate::app::{App, Focus, Mode, View};

/// Returns Ok(true) when the app should exit cleanly.
pub fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    match app.mode.clone() {
        Mode::Browse => handle_key_browse(app, key),
        Mode::PickApp { .. } | Mode::PickMime { .. } => handle_key_picker(app, key),
        Mode::ConfirmQuit => Ok(handle_key_confirm_quit(app, key)),
        Mode::Help => Ok(handle_key_help(app, key)),
    }
}

// ───────────── Browse ────────────────────────────────────────────────────────

fn handle_key_browse(app: &mut App, key: KeyEvent) -> Result<bool> {
    // Mode-changing globals first.
    match (key.code, key.modifiers) {
        (KeyCode::Char('s'), KeyModifiers::CONTROL) => {
            do_save(app);
            return Ok(false);
        }
        (KeyCode::Char('z'), KeyModifiers::CONTROL) => {
            if app.is_dirty() {
                app.action_discard();
                app.set_flash("Discarded pending edits.");
            }
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
                    app.open_pick_mime(app_id);
                }
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
        KeyCode::Right => {
            // Always allow focusing the right pane — even when empty — so the
            // user has a reliable way to reach 'a' (add/picker) on items that
            // currently have no associations.
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
    match app.view {
        View::ByMime => {
            let Some(m) = app.currently_selected_mime() else {
                return 0;
            };
            let mime_id = m.id.clone();
            app.effective_associations_for(&mime_id).len()
        }
        View::ByApp => {
            let Some(a) = app.currently_selected_app() else {
                return 0;
            };
            app.mime_list_for_app(&a.id).len()
        }
    }
}

fn reset_left_selection(app: &mut App) {
    app.selected_left = 0;
    app.selected_right = 0;
}

/// Extract the (mime, app_id, display_label) currently targeted by the right
/// pane selection — view-aware:
///   * by-mime: mime is the left selection, app_id is right pane's selected
///     entry from `effective_associations_for(mime)`.
///   * by-app: app_id is the left selection, mime is right pane's selected
///     entry from `mime_list_for_app(app)`.
fn target_pair(app: &App) -> Option<(String, String, String)> {
    match app.view {
        View::ByMime => {
            let mime = app.currently_selected_mime()?;
            let mime_id = mime.id.clone();
            let assoc = app.effective_associations_for(&mime_id);
            let entry = assoc.get(app.selected_right)?;
            Some((mime_id, entry.id.clone(), entry.name.clone()))
        }
        View::ByApp => {
            let a = app.currently_selected_app()?;
            let app_id = a.id.clone();
            let list = app.mime_list_for_app(&app_id);
            let (mime, _rel) = list.get(app.selected_right)?;
            Some((mime.id.clone(), app_id, mime.id.clone()))
        }
    }
}

fn action_set_default(app: &mut App) {
    let Some((mime_id, app_id, label)) = target_pair(app) else {
        return;
    };
    app.action_set_default(&mime_id, &app_id);
    app.set_flash(match app.view {
        View::ByMime => format!("Default for {} → {}", mime_id, label),
        View::ByApp => format!("{} now default for {}", label, mime_id),
    });
}

fn action_remove(app: &mut App) {
    let Some((mime_id, app_id, label)) = target_pair(app) else {
        return;
    };
    app.action_remove_assoc(&mime_id, &app_id);
    app.set_flash(format!("Removed: {} ↔ {}", mime_id, label));

    // Keep right-pane selection in bounds.
    let new_count = right_pane_count(app);
    if new_count == 0 {
        app.focus = Focus::Left;
        app.selected_right = 0;
    } else if app.selected_right >= new_count {
        app.selected_right = new_count - 1;
    }
}

fn action_clear_default(app: &mut App) {
    let Some((mime_id, _, _)) = target_pair(app) else {
        return;
    };
    app.action_clear_default(&mime_id);
    app.set_flash(format!("Cleared default for {}", mime_id));
}

fn do_save(app: &mut App) {
    if !app.is_dirty() {
        app.set_flash("Nothing to save.");
        return;
    }
    match app.save() {
        Ok((n, shadowed)) => {
            let mut msg = format!("Saved {} edit(s) to mimeapps.list", n);
            if !shadowed.is_empty() {
                msg.push_str(&format!(
                    " — {} default{} shadowed by a per-desktop override (see detail pane)",
                    shadowed.len(),
                    if shadowed.len() == 1 { "" } else { "s" },
                ));
            }
            app.set_flash(msg);
        }
        Err(e) => app.set_flash(format!("Save failed: {}", e)),
    }
}

// ───────────── Picker ────────────────────────────────────────────────────────

/// Fixed page size for picker nav. The picker's list height isn't tracked
/// (we'd need a `pick_list_rect: Option<Rect>` plumbed through draw), and a
/// generic chunk works fine in practice since users can page repeatedly.
const PICKER_PAGE: usize = 10;


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

        // Toggle: Space or Enter on the cursor row (or the marked range).
        // Picker stays open so the user can rapidly toggle many entries.
        (KeyCode::Char(' '), KeyModifiers::NONE) | (KeyCode::Enter, _) => {
            app.picker_apply_toggle();
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
        _ => {}
    }
    forward_to_input(&mut app.pick_input, key);
    // Filter changed → the previous mark refers to a different visual row.
    // Easier to clear than to track which mime/app it had pointed at.
    app.pick_selected = 0;
    app.pick_mark = None;
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

fn handle_key_help(app: &mut App, _key: KeyEvent) -> bool {
    // Any key dismisses the help overlay.
    app.mode = Mode::Browse;
    false
}

// ───────────── ConfirmQuit ───────────────────────────────────────────────────

fn handle_key_confirm_quit(app: &mut App, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
            app.action_discard();
            true
        }
        KeyCode::Char('s') | KeyCode::Char('S') => {
            do_save(app);
            !app.is_dirty() // only quit if save succeeded
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

// ───────────── Mouse (Phase 3: same scope as Phase 2) ────────────────────────

pub fn handle_mouse(app: &mut App, ev: MouseEvent) -> Result<bool> {
    if app.mode != Mode::Browse {
        return Ok(false);
    }
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
            if let Some(rect) = app.left_rect {
                if rect_contains(rect, col, row) {
                    for _ in 0..3 {
                        if app.selected_left > 0 {
                            app.selected_left -= 1;
                        }
                    }
                    return Ok(false);
                }
            }
        }
        MouseEventKind::ScrollDown => {
            if let Some(rect) = app.left_rect {
                if rect_contains(rect, col, row) {
                    let count = left_pane_count(app);
                    for _ in 0..3 {
                        if count > 0 && app.selected_left + 1 < count {
                            app.selected_left += 1;
                        }
                    }
                    return Ok(false);
                }
            }
        }
        _ => {}
    }
    Ok(false)
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
