use crate::app::{App, Focus, Mode, View};
use crate::config::{MimeTuiConfig, Theme};
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

/// One atomic hint — wrapping never breaks inside a chunk. A chunk is
/// typically `(key, label)` like `"d"` + `":set default"`.
type Chunk = Vec<Span<'static>>;

pub fn required_height(app: &App, config: &MimeTuiConfig, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let lines = if app.flash_message().is_some() {
        // Flash messages are user-facing sentences — fine to wrap on words.
        let w = build_flash_line(app, config).width() as u16;
        if w == 0 { 1 } else { ((w + width - 1) / width).max(1) }
    } else {
        layout_chunks(build_chunks(app, config), width).len() as u16
    };
    lines.max(1).min(MAX_STATUS_ROWS)
}

pub fn draw(f: &mut Frame, app: &App, area: Rect, config: &MimeTuiConfig) {
    let text = crate::ui::layout::theme_text_style(config);
    if app.flash_message().is_some() {
        f.render_widget(
            Paragraph::new(build_flash_line(app, config))
                .style(text)
                .wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let lines = layout_chunks(build_chunks(app, config), area.width);
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

    // Header — view label + pending count if dirty. Always renders first;
    // can wrap onto its own line on extremely narrow terminals.
    let dirty_suffix = if app.is_dirty() {
        format!(" * {} pending", app.pending.count())
    } else {
        String::new()
    };
    chunks.push(vec![Span::raw(format!(
        " [{}]{} ",
        view_label(app.view),
        dirty_suffix
    ))]);

    match &app.mode {
        Mode::Browse => browse_chunks(&mut chunks, app, key, dim),
        Mode::PickApp { .. } | Mode::PickMime { .. } => {
            chunks.push(kv_chunk("Space", ":toggle", key, dim));
            chunks.push(kv_chunk("Enter", ":accept", key, dim));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk("Esc", ":cancel", key, dim));
        }
        Mode::ConfirmQuit => {
            chunks.push(kv_chunk("y", ":discard", key, dim));
            chunks.push(kv_chunk("s", ":save", key, dim));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk("n", ":cancel", key, dim));
        }
        Mode::Help => {
            chunks.push(kv_chunk("Space/PgDn", ":page down", key, dim));
            chunks.push(kv_chunk("b/PgUp", ":page up", key, dim));
            chunks.push(kv_chunk("↑/↓", ":line", key, dim));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk("q/Esc", ":close", key, dim));
        }
        Mode::ConflictResolve { .. } => {
            chunks.push(kv_chunk("r", ":reload", key, dim));
            chunks.push(kv_chunk("o", ":overwrite", key, dim));
            chunks.push(kv_chunk("m", ":merge", key, dim));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk("c/Esc", ":cancel", key, dim));
        }
        Mode::ThemePick { .. } => {
            chunks.push(kv_chunk("↑↓", ":preview", key, dim));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk("Enter", ":keep", key, dim));
            chunks.push(sep_chunk(dim));
            chunks.push(kv_chunk("Esc", ":cancel", key, dim));
        }
    }

    chunks
}

fn browse_chunks(chunks: &mut Vec<Chunk>, app: &App, key: Style, dim: Style) {
    // Group 1 — navigation: switch view + move between panes.
    chunks.push(kv_chunk("Tab", ":view", key, dim));
    if app.focus == Focus::Right {
        chunks.push(kv_chunk("←", ":back", key, dim));
    } else {
        chunks.push(kv_chunk("→", ":edit", key, dim));
    }

    // Group 2 — operations on the current row (only meaningful when the
    // right pane is focused, since that's where edits happen).
    if app.focus == Focus::Right {
        let (remove_label, add_label) = match app.view {
            View::ByMime => ("remove app", "add app"),
            View::ByApp => ("remove mime", "add mime"),
        };
        chunks.push(sep_chunk(dim));
        chunks.push(kv_chunk_owned("a", format!(":{}", add_label), key, dim));
        chunks.push(kv_chunk("d", ":set default", key, dim));
        chunks.push(kv_chunk("c", ":clear default", key, dim));
        chunks.push(kv_chunk_owned("r", format!(":{}", remove_label), key, dim));
    }

    // Group 3 — misc: save/discard, then top-level utilities. Ctrl-T is a
    // top-level affordance — hide it on the right pane to keep that hint
    // bar focused on row operations.
    chunks.push(sep_chunk(dim));
    chunks.push(kv_chunk("Ctrl-S", ":save", key, dim));
    if app.focus == Focus::Left {
        chunks.push(kv_chunk("Ctrl-T", ":theme", key, dim));
    }
    chunks.push(kv_chunk("?", ":help", key, dim));
    chunks.push(kv_chunk("Esc", ":quit", key, dim));
}

fn kv_chunk(k: &'static str, v: &'static str, key: Style, dim: Style) -> Chunk {
    vec![Span::styled(k, key), Span::styled(v, dim)]
}

fn kv_chunk_owned(k: &'static str, v: String, key: Style, dim: Style) -> Chunk {
    vec![Span::styled(k, key), Span::styled(v, dim)]
}

/// A thin vertical bar used to visually separate groups of hints
/// (navigation │ operations │ misc). Treated as a full chunk so the layout
/// engine keeps it whole and surrounds it with the regular two-space
/// chunk separators on either side.
fn sep_chunk(dim: Style) -> Chunk {
    vec![Span::styled("│", dim)]
}

/// Pack chunks left-to-right into lines of width `width`. A chunk never
/// splits across lines — when the next chunk would overflow, we break before
/// it. The header chunk is always placed first; subsequent overflow wraps
/// down and is indented so it aligns with the first hint on line 1.
fn layout_chunks(chunks: Vec<Chunk>, width: u16) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_w: usize = 0;
    let width_us = width.max(1) as usize;
    let sep_w = CHUNK_SEPARATOR.chars().count();

    // The width of the header chunk + separator is also where the first hint
    // starts on line 1. Use it as the indent on lines 2+ so wrapped hints
    // align under the first hint rather than starting at column 0.
    let raw_indent_w = chunks
        .first()
        .map(|c| c.iter().map(|s| s.width()).sum::<usize>() + sep_w)
        .unwrap_or(0);
    // Skip the indent if it'd crowd hints out on a very narrow terminal.
    let indent_w = if raw_indent_w * 3 > width_us {
        0
    } else {
        raw_indent_w
    };
    let indent_str: String = " ".repeat(indent_w);

    for chunk in chunks {
        let chunk_w: usize = chunk.iter().map(|s| s.width()).sum();
        let needed = if current.is_empty() { chunk_w } else { sep_w + chunk_w };

        if !current.is_empty() && current_w + needed > width_us {
            lines.push(Line::from(std::mem::take(&mut current)));
            current_w = 0;
        }
        if current.is_empty() {
            // On lines 2+ prepend the indent so this chunk lines up under
            // the first hint of line 1.
            if !lines.is_empty() && indent_w > 0 {
                current.push(Span::raw(indent_str.clone()));
                current_w = indent_w;
            }
            current.extend(chunk);
            current_w += chunk_w;
        } else {
            current.push(Span::raw(CHUNK_SEPARATOR));
            current.extend(chunk);
            current_w += sep_w + chunk_w;
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(current));
    }
    if lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines
}

fn view_label(v: View) -> &'static str {
    match v {
        View::ByMime => "by-mime",
        View::ByApp => "by-app",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a chunk from a single string of `width` chars. Saves boilerplate
    /// in the layout tests below where the styling is irrelevant.
    fn chunk(text: &str) -> Chunk {
        vec![Span::raw(text.to_string())]
    }

    /// Concatenate a Line's spans into a plain String so tests can assert on
    /// the visible content.
    fn line_text(line: &Line<'_>) -> String {
        line.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn single_chunk_fits_on_one_line() {
        let lines = layout_chunks(vec![chunk("hello")], 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "hello");
    }

    #[test]
    fn multiple_chunks_join_with_two_space_separator() {
        let lines = layout_chunks(
            vec![chunk("[h]"), chunk("Tab"), chunk("Esc")],
            80,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "[h]  Tab  Esc");
    }

    #[test]
    fn wraps_when_next_chunk_would_overflow() {
        // Header 6 wide; each hint 3 wide; sep 2 wide. indent_w = 6+2 = 8.
        // width=30 → indent_w*3=24 ≤ 30, so indent IS applied on wrap.
        // line 1: "[hdr]   xxx  yyy  zzz  www" = 6+2+3+2+3+2+3+2+3 = 26 (fits)
        //   adding "vvv" needs +2+3 = +5 → 31 > 30 → wrap before vvv.
        // line 2: 8 spaces + "vvv  uuu" = 8 + 3+2+3 = 16.
        let lines = layout_chunks(
            vec![
                chunk("[hdr] "),
                chunk("xxx"),
                chunk("yyy"),
                chunk("zzz"),
                chunk("www"),
                chunk("vvv"),
                chunk("uuu"),
            ],
            30,
        );
        assert_eq!(lines.len(), 2, "expected exactly 2 lines, got {:?}",
            lines.iter().map(line_text).collect::<Vec<_>>());
        assert_eq!(line_text(&lines[0]), "[hdr]   xxx  yyy  zzz  www");
        assert_eq!(line_text(&lines[1]), "        vvv  uuu");
    }

    #[test]
    fn wrap_never_breaks_inside_a_chunk() {
        // A chunk like ["a", ":add mime"] is one atomic unit — wrap must
        // keep it whole even if `a:add` would fit on the previous line.
        let lines = layout_chunks(
            vec![
                chunk("[h] "),
                chunk("filler"),
                vec![Span::raw("a"), Span::raw(":add mime")],
            ],
            14,
        );
        // header 4 + sep 2 + filler 6 = 12; +2 sep + 10 ("a:add mime") = 24 > 14, wrap.
        assert!(lines.len() >= 2);
        // Whichever line carries the atomic chunk, its text contains the
        // full "a:add mime" with no break.
        let joined: String = lines.iter().map(line_text).collect();
        assert!(joined.contains("a:add mime"));
    }

    #[test]
    fn wrapped_lines_indent_to_align_with_first_hint() {
        // Header chunk is 9 wide ("[by-mime]"). indent_w = 9 + 2 = 11.
        // With width 50, indent_w*3 = 33 ≤ 50 → indent applied.
        let lines = layout_chunks(
            vec![
                chunk("[by-mime]"),
                chunk("Tab: view"),
                chunk("d:set default"),
                chunk("r:remove app"),
                chunk("c:clear default"),
                chunk("a:add app"),
                chunk("Ctrl-S:save"),
            ],
            50,
        );
        assert!(lines.len() >= 2);
        // Every line after the first starts with exactly 11 spaces of indent.
        for (i, line) in lines.iter().enumerate().skip(1) {
            let text = line_text(line);
            assert!(
                text.starts_with("           "), // 11 spaces
                "line {} should start with 11-space indent: {:?}",
                i,
                text
            );
            // …followed by a non-space (the next hint, flush against the indent).
            assert_ne!(text.chars().nth(11), Some(' '),
                "line {} should have non-space after the indent: {:?}", i, text);
        }
    }

    #[test]
    fn very_narrow_terminal_drops_the_indent() {
        // indent_w = 14 + 2 = 16; width 18 → 16*3 = 48 > 18 → no indent.
        let lines = layout_chunks(
            vec![
                chunk("[long-header] "),
                chunk("a"),
                chunk("b"),
                chunk("c"),
            ],
            18,
        );
        // Every wrapped line starts at column 0 (no leading spaces).
        for (i, line) in lines.iter().enumerate().skip(1) {
            let text = line_text(line);
            assert!(
                !text.starts_with(' '),
                "line {} should NOT be indented on a narrow terminal: {:?}",
                i,
                text
            );
        }
    }

    #[test]
    fn chunk_wider_than_width_still_renders() {
        // A header chunk that overflows the terminal should be placed as-is
        // (the terminal will clip it), not dropped.
        let lines = layout_chunks(
            vec![chunk("this header is wider than the width"), chunk("x")],
            10,
        );
        assert!(line_text(&lines[0]).contains("wider than"));
        assert!(lines.len() >= 2);
    }

    #[test]
    fn empty_chunks_yields_one_blank_line() {
        // Sanity: layout_chunks must always return at least one line so the
        // status strip has something to render.
        let lines = layout_chunks(vec![], 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "");
    }

    #[test]
    fn zero_width_does_not_panic() {
        // The width arg is `u16` so 0 is reachable in pathological cases.
        // We don't care what the layout looks like, only that we don't
        // divide-by-zero or hang.
        let _ = layout_chunks(vec![chunk("a"), chunk("b")], 0);
    }

    #[test]
    fn first_chunk_never_gets_leading_separator() {
        // Even on wrapped lines, the indent (spaces) replaces any chunk
        // separator — we should never emit "  " then the chunk on a wrap-down.
        let lines = layout_chunks(
            vec![chunk("[h] "), chunk("aaaaaaaaaa"), chunk("bbbbbbbbbb")],
            14,
        );
        for line in &lines {
            let text = line_text(line);
            assert!(
                !text.trim_start().starts_with(' '),
                "line should not begin with the chunk separator: {:?}",
                text
            );
        }
    }
}
