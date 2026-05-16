use crate::app::{App, Focus, Mode, View};
use crate::config::{MimeTuiConfig, Theme};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};

/// Hard cap on how many rows we let the status bar take. Without this a bug
/// in measurement could push the main content off-screen on tiny terminals.
const MAX_STATUS_ROWS: u16 = 4;

/// Inserted between adjacent chunks on the same line. Two spaces gives
/// breathing room without looking sparse on wide terminals.
const CHUNK_SEPARATOR: &str = "  ";

/// One atomic hint — wrapping never breaks inside a chunk. Each chunk also
/// optionally carries the key event the click should synthesise, so the
/// mouse handler can re-use the regular key-dispatch path.
#[derive(Clone)]
struct Chunk {
    spans: Vec<Span<'static>>,
    action: Option<KeyEvent>,
}

impl Chunk {
    fn width(&self) -> usize {
        self.spans.iter().map(|s| s.width()).sum()
    }
}

pub fn required_height(app: &App, config: &MimeTuiConfig, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let lines = if app.flash_message().is_some() {
        // Flash messages are user-facing sentences — fine to wrap on words.
        let w = build_flash_line(app, config).width() as u16;
        if w == 0 { 1 } else { ((w + width - 1) / width).max(1) }
    } else {
        let (lines, _) = layout_chunks(build_chunks(app, config), width);
        lines.len() as u16
    };
    lines.max(1).min(MAX_STATUS_ROWS)
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect, config: &MimeTuiConfig) {
    let text = crate::ui::layout::theme_text_style(config);
    if app.flash_message().is_some() {
        // Flash messages aren't clickable; clear the previous frame's
        // status-bar hit regions so an old hint can't accidentally fire
        // through the toast.
        app.status_clickables.clear();
        f.render_widget(
            Paragraph::new(build_flash_line(app, config))
                .style(text)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let (lines, positions) = layout_chunks(build_chunks(app, config), area.width);

    // Translate per-chunk (line, col, width, action) into absolute-screen
    // rects so the mouse handler can hit-test directly.
    app.status_clickables = positions
        .into_iter()
        .map(|(line_idx, col, w, action)| {
            let rect = Rect::new(
                area.x.saturating_add(col as u16),
                area.y.saturating_add(line_idx as u16),
                w as u16,
                1,
            );
            (rect, action)
        })
        .collect();

    // Plain Paragraph — no `.wrap()` because we've already wrapped manually
    // at chunk boundaries.
    f.render_widget(Paragraph::new(lines).style(text), area);
}

fn build_flash_line(app: &App, config: &MimeTuiConfig) -> Line<'static> {
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let msg = app.flash_message().unwrap_or("");
    Line::from(vec![Span::styled(format!(" {} ", msg), key)])
}

fn build_chunks(app: &App, config: &MimeTuiConfig) -> Vec<Chunk> {
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let mut chunks: Vec<Chunk> = Vec::new();

    // Header — view label + pending count if dirty. Clicking it toggles
    // the view (same as Tab), so the `[by-mime]`/`[by-app]` indicator
    // doubles as a view-switch button.
    let dirty_suffix = if app.is_dirty() {
        format!(" * {} pending", app.pending.count())
    } else {
        String::new()
    };
    chunks.push(Chunk {
        spans: vec![Span::raw(format!(
            " [{}]{} ",
            view_label(app.view),
            dirty_suffix
        ))],
        action: Some(key_press(KeyCode::Tab)),
    });

    match &app.mode {
        Mode::Browse => browse_chunks(&mut chunks, app, key, dim),
        Mode::PickApp { .. } | Mode::PickMime { .. } => {
            chunks.push(kv_chunk(
                "Space", ":toggle", key, dim, key_press(KeyCode::Char(' ')),
            ));
            chunks.push(kv_chunk(
                "Enter", ":accept", key, dim, key_press(KeyCode::Enter),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "Esc", ":cancel", key, dim, key_press(KeyCode::Esc),
            ));
        }
        Mode::ConfirmQuit => {
            chunks.push(kv_chunk(
                "y", ":discard", key, dim, key_press(KeyCode::Char('y')),
            ));
            chunks.push(kv_chunk(
                "s", ":save", key, dim, key_press(KeyCode::Char('s')),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "n", ":cancel", key, dim, key_press(KeyCode::Char('n')),
            ));
        }
        Mode::Help => {
            // Click directions match the natural reading of the hint:
            // "page down" clicks page down, "page up" clicks page up.
            chunks.push(kv_chunk(
                "Space/PgDn", ":page down", key, dim,
                key_press(KeyCode::PageDown),
            ));
            chunks.push(kv_chunk(
                "b/PgUp", ":page up", key, dim,
                key_press(KeyCode::PageUp),
            ));
            chunks.push(kv_chunk(
                "↑/↓", ":line", key, dim, key_press(KeyCode::Down),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "q/Esc", ":close", key, dim, key_press(KeyCode::Esc),
            ));
        }
        Mode::ConflictResolve { .. } => {
            chunks.push(kv_chunk(
                "r", ":reload", key, dim, key_press(KeyCode::Char('r')),
            ));
            chunks.push(kv_chunk(
                "o", ":overwrite", key, dim, key_press(KeyCode::Char('o')),
            ));
            chunks.push(kv_chunk(
                "m", ":merge", key, dim, key_press(KeyCode::Char('m')),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "c/Esc", ":cancel", key, dim, key_press(KeyCode::Char('c')),
            ));
        }
        Mode::ThemePick { .. } => {
            chunks.push(kv_chunk(
                "↑↓", ":preview", key, dim, key_press(KeyCode::Down),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "Enter", ":keep", key, dim, key_press(KeyCode::Enter),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "Esc", ":cancel", key, dim, key_press(KeyCode::Esc),
            ));
        }
        Mode::ConfirmSave => {
            chunks.push(kv_chunk(
                "↑↓/PgUp/PgDn", ":scroll", key, dim,
                key_press(KeyCode::PageDown),
            ));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk(
                "Enter/y", ":save", key, dim, key_press(KeyCode::Enter),
            ));
            chunks.push(kv_chunk(
                "Esc/n", ":cancel", key, dim, key_press(KeyCode::Esc),
            ));
        }
    }

    chunks
}

fn browse_chunks(chunks: &mut Vec<Chunk>, app: &App, key: Style, dim: Style) {
    // Group 1 — navigation: switch view + move between panes.
    chunks.push(kv_chunk("Tab", ":view", key, dim, key_press(KeyCode::Tab)));
    if app.focus == Focus::Right {
        chunks.push(kv_chunk(
            "←", ":back", key, dim, key_press(KeyCode::Left),
        ));
    } else {
        // `→:edit` is deliberately *not* clickable — clicking a row in
        // the right pane already focuses it, so a dedicated button would
        // be redundant. Kept as a hint for users on keyboard only.
        chunks.push(no_action_chunk("→", ":edit", key, dim));
    }

    // Group 2 — operations on the current row (only meaningful when the
    // right pane is focused, since that's where edits happen).
    //
    // When the target app is a phantom (uninstalled), `d` and `a` (in
    // by-app) would just plant fresh orphan entries — exactly what the
    // red-row UX is meant to surface and clean up. Drop those chunks so
    // they're neither shown nor click-fireable; `c` and `r` stay (both
    // are valid cleanup actions on a phantom target).
    if app.focus == Focus::Right {
        let (remove_label, add_label) = match app.view {
            View::ByMime => ("remove app", "add app"),
            View::ByApp => ("remove mime", "add mime"),
        };
        let target_phantom = app.current_target_is_phantom();
        // `a` in by-mime operates on the mime context (opens the picker
        // for the selected mime), so it's valid regardless of whether
        // the right-pane row is a phantom. In by-app it operates on the
        // selected app — suppress when that app is a phantom.
        let show_add = match app.view {
            View::ByMime => true,
            View::ByApp => !target_phantom,
        };
        let show_set_default = !target_phantom;

        chunks.push(sep_chunk(dim));
        if show_add {
            chunks.push(kv_chunk_owned(
                "a",
                format!(":{}", add_label),
                key,
                dim,
                key_press(KeyCode::Char('a')),
            ));
        }
        if show_set_default {
            chunks.push(kv_chunk(
                "d", ":set default", key, dim, key_press(KeyCode::Char('d')),
            ));
        }
        chunks.push(kv_chunk(
            "c", ":clear default", key, dim, key_press(KeyCode::Char('c')),
        ));
        chunks.push(kv_chunk_owned(
            "r",
            format!(":{}", remove_label),
            key,
            dim,
            key_press(KeyCode::Char('r')),
        ));
    }

    // Group 3 — misc: save/discard, then top-level utilities. Ctrl-T is a
    // top-level affordance — hide it on the right pane to keep that hint
    // bar focused on row operations.
    chunks.push(sep_chunk(dim));
    chunks.push(kv_chunk(
        "Ctrl-S",
        ":save",
        key,
        dim,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    ));
    if app.focus == Focus::Left {
        chunks.push(kv_chunk(
            "Ctrl-T",
            ":theme",
            key,
            dim,
            KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL),
        ));
    }
    chunks.push(kv_chunk(
        "?", ":help", key, dim, key_press(KeyCode::Char('?')),
    ));
    chunks.push(kv_chunk(
        "Esc", ":quit", key, dim, key_press(KeyCode::Esc),
    ));
}

fn key_press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn kv_chunk(
    k: &'static str,
    v: &'static str,
    key: Style,
    dim: Style,
    action: KeyEvent,
) -> Chunk {
    Chunk {
        spans: vec![Span::styled(k, key), Span::styled(v, dim)],
        action: Some(action),
    }
}

fn kv_chunk_owned(
    k: &'static str,
    v: String,
    key: Style,
    dim: Style,
    action: KeyEvent,
) -> Chunk {
    Chunk {
        spans: vec![Span::styled(k, key), Span::styled(v, dim)],
        action: Some(action),
    }
}

/// A `kv` chunk that's intentionally non-clickable — used for the `→:edit`
/// hint where clicking a row already focuses the right pane, so a button
/// would be redundant.
fn no_action_chunk(
    k: &'static str,
    v: &'static str,
    key: Style,
    dim: Style,
) -> Chunk {
    Chunk {
        spans: vec![Span::styled(k, key), Span::styled(v, dim)],
        action: None,
    }
}

/// A thin vertical bar used to visually separate groups of hints
/// (navigation │ operations │ misc). Treated as a full chunk so the layout
/// engine keeps it whole and surrounds it with the regular two-space
/// chunk separators on either side.
fn sep_chunk(dim: Style) -> Chunk {
    Chunk {
        spans: vec![Span::styled("│", dim)],
        action: None,
    }
}

/// Per-chunk position emitted by `layout_chunks` alongside the rendered
/// lines: `(line_idx, col_start, chunk_width, action)`. Only chunks with
/// a defined `action` produce entries.
type ChunkPos = (usize, usize, usize, KeyEvent);

/// Pack chunks left-to-right into lines of width `width`. A chunk never
/// splits across lines — when the next chunk would overflow, we break before
/// it. The header chunk is always placed first; subsequent overflow wraps
/// down and is indented so it aligns with the first hint on line 1.
///
/// Also returns the position of each *clickable* chunk so the caller can
/// translate to screen rects for mouse hit-testing.
fn layout_chunks(
    chunks: Vec<Chunk>,
    width: u16,
) -> (Vec<Line<'static>>, Vec<ChunkPos>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_w: usize = 0;
    let width_us = width.max(1) as usize;
    let sep_w = CHUNK_SEPARATOR.chars().count();

    // Header indent = width of header chunk + separator. Used to align
    // wrapped lines under the first hint on line 1.
    let raw_indent_w = chunks
        .first()
        .map(|c| c.width() + sep_w)
        .unwrap_or(0);
    let indent_w = if raw_indent_w * 3 > width_us {
        0
    } else {
        raw_indent_w
    };
    let indent_str: String = " ".repeat(indent_w);

    let mut positions: Vec<ChunkPos> = Vec::new();

    for chunk in chunks {
        let chunk_w = chunk.width();
        let needed = if current.is_empty() { chunk_w } else { sep_w + chunk_w };

        if !current.is_empty() && current_w + needed > width_us {
            lines.push(Line::from(std::mem::take(&mut current)));
            current_w = 0;
        }
        let start_col;
        if current.is_empty() {
            // On lines 2+ prepend the indent so this chunk lines up under
            // the first hint of line 1.
            if !lines.is_empty() && indent_w > 0 {
                current.push(Span::raw(indent_str.clone()));
                current_w = indent_w;
            }
            start_col = current_w;
            current.extend(chunk.spans);
            current_w += chunk_w;
        } else {
            current.push(Span::raw(CHUNK_SEPARATOR));
            current_w += sep_w;
            start_col = current_w;
            current.extend(chunk.spans);
            current_w += chunk_w;
        }
        if let Some(action) = chunk.action {
            positions.push((lines.len(), start_col, chunk_w, action));
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    (lines, positions)
}

fn view_label(v: View) -> &'static str {
    match v {
        View::ByMime => "by-mime",
        View::ByApp => "by-app",
    }
}

#[cfg(test)]
mod tests;
