use crate::app::{App, Focus, Mode, Relation};
use crate::config::{MimeTuiConfig, Theme};
use crate::ui::layout;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use tui_input::Input;

pub fn draw(f: &mut Frame, app: &mut App, config: &MimeTuiConfig) {
    let secondary = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let (title, items) = match app.mode.clone() {
        Mode::PickApp { for_mime } => {
            let visible: Vec<crate::model::DesktopApp> =
                app.picker_visible_apps().into_iter().cloned().collect();
            let items: Vec<Line<'static>> = visible
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let rel = app.relation_of(&for_mime, &a.id);
                    let mstyle = marker_style(rel, config);
                    build_row(
                        rel,
                        in_marked_range(app, i),
                        &a.name,
                        &a.id,
                        secondary,
                        mstyle,
                    )
                })
                .collect();
            (format!(" Toggle apps for {} ", for_mime), items)
        }
        Mode::PickMime { for_app } => {
            let visible: Vec<crate::model::MimeType> =
                app.picker_visible_mimes().into_iter().cloned().collect();
            let items: Vec<Line<'static>> = visible
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let rel = app.relation_of(&m.id, &for_app);
                    let mstyle = marker_style(rel, config);
                    build_row(
                        rel,
                        in_marked_range(app, i),
                        &m.id,
                        &m.description,
                        secondary,
                        mstyle,
                    )
                })
                .collect();
            (format!(" Toggle mime types for {} ", for_app), items)
        }
        _ => return,
    };

    let area = centered_rect(f.area(), 70, 70);
    f.render_widget(Clear, area);

    let border = Theme::parse_color(&config.colors.focus);
    let key = Style::default().add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let text = layout::theme_text_style(config);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border))
        .style(text);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    render_picker_search(f, chunks[0], &app.pick_input, config);

    // Compute max hscroll based on the longest row's overflow vs the list's
    // inner content width. layout::render_list_lines adds 2 borders + 2 cells
    // of left/right padding, so subtract 4.
    let inner_width = chunks[1].width.saturating_sub(4);
    let max_line_w = items.iter().map(|l| l.width() as u16).max().unwrap_or(0);
    let max_hscroll = max_line_w.saturating_sub(inner_width);
    let hscroll = app.pick_hscroll.min(max_hscroll);
    app.pick_hscroll = hscroll;

    let visible_items: Vec<Line<'static>> = items
        .iter()
        .map(|l| layout::scroll_line(l, hscroll as usize, 2))
        .collect();

    let clamped = app.pick_selected.min(visible_items.len().saturating_sub(1));
    layout::render_list_lines(
        f,
        chunks[1],
        " Candidates ",
        &visible_items,
        Some(clamped),
        true,
        config,
        &mut app.pick_list_state,
    );
    // Remember the list rect so mouse handlers can hit-test it.
    app.pick_list_rect = Some(chunks[1]);
    layout::render_list_hscrollbar(
        f,
        chunks[1],
        max_line_w as usize,
        hscroll as usize,
        config,
    );

    let mark_active = app.pick_mark.is_some();
    let hint = if mark_active {
        Line::from(vec![
            Span::styled(" Space", key),
            Span::styled(": toggle range  ", dim),
            Span::styled("Enter", key),
            Span::styled(": accept  ", dim),
            Span::styled("Ctrl-Space", key),
            Span::styled(": clear mark  ", dim),
            Span::styled("Esc", key),
            Span::styled(": close", dim),
        ])
    } else {
        Line::from(vec![
            Span::styled(" Space", key),
            Span::styled(": toggle  ", dim),
            Span::styled("Enter", key),
            Span::styled(": accept  ", dim),
            Span::styled("Ctrl-Space", key),
            Span::styled(": set mark  ", dim),
            Span::styled("Esc", key),
            Span::styled(": close", dim),
        ])
    };
    f.render_widget(Paragraph::new(hint).style(text), chunks[2]);
}

/// Build a single picker row as a Line with four logical parts:
///
/// `[rel] [mark+sep] primary    secondary`
///
/// Span 0 carries the relation marker styled with `marker_style` so each row
/// gets a per-relation colour (★ / ✓ / · all distinct). Span 1 is the mark
/// indicator + trailing space, raw, so the bg-only emacs-mark glyph keeps
/// its own (uncoloured) look. Together these two spans form the 3-column
/// prefix that `layout::scroll_line` pins when the row is horizontally
/// scrolled.
fn build_row(
    rel: Option<Relation>,
    marked: bool,
    primary: &str,
    secondary: &str,
    secondary_style: Style,
    marker_style: Style,
) -> Line<'static> {
    let rel_char = match rel {
        Some(Relation::Default) => '★',
        Some(Relation::Associated) => '✓',
        Some(Relation::DeclaredOnly) => '·',
        None => ' ',
    };
    let mark_char = if marked { '▌' } else { ' ' };

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(rel_char.to_string(), marker_style),
        Span::raw(format!("{} ", mark_char)),
        Span::raw(primary.to_string()),
    ];
    if !secondary.is_empty() {
        spans.push(Span::raw("    "));
        spans.push(Span::styled(secondary.to_string(), secondary_style));
    }
    Line::from(spans)
}

/// Pick the per-marker foreground colour. `None` (no relationship) gets
/// the default style so the leading column of an unrelated row is just an
/// uncoloured space.
fn marker_style(rel: Option<Relation>, config: &MimeTuiConfig) -> Style {
    let colour_field = match rel {
        Some(Relation::Default) => &config.colors.marker_default,
        Some(Relation::Associated) => &config.colors.marker_associated,
        Some(Relation::DeclaredOnly) => &config.colors.marker_declared_only,
        None => return Style::default(),
    };
    Style::default().fg(Theme::parse_color(colour_field))
}

fn in_marked_range(app: &App, i: usize) -> bool {
    match app.pick_mark {
        None => false,
        Some(mark) => {
            let lo = mark.min(app.pick_selected);
            let hi = mark.max(app.pick_selected);
            i >= lo && i <= hi
        }
    }
}

fn render_picker_search(f: &mut Frame, area: Rect, input: &Input, config: &MimeTuiConfig) {
    layout::render_search_bar(f, area, input, Focus::Search, config);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'_>) -> String {
        line.iter().map(|s| s.content.as_ref()).collect()
    }

    fn sample_row() -> Line<'static> {
        // Mimic a real picker row: [marker][space] primary    secondary
        Line::from(vec![
            Span::raw("★ "),                    // span 0: prefix (3 cols if we count the ★)
            Span::raw("text/html"),             // span 1: primary
            Span::raw("    "),                  // span 2: separator
            Span::raw("HTML document"),         // span 3: secondary
        ])
    }

    #[test]
    fn zero_hscroll_returns_unchanged_line() {
        let line = sample_row();
        let out = layout::scroll_line(&line, 0, 1);
        assert_eq!(line_text(&out), line_text(&line));
    }

    #[test]
    fn prefix_stays_pinned_when_scrolling() {
        // Scroll past the primary text entirely.
        let line = sample_row();
        let out = layout::scroll_line(&line, 100, 1);
        // First span must still be the prefix.
        let first = &out.spans[0];
        assert_eq!(first.content.as_ref(), "★ ");
    }

    #[test]
    fn hscroll_drops_leading_chars_after_prefix() {
        let line = sample_row();
        // hscroll = 5 → skip 5 chars from "text/html" + onward.
        // "text/html" is 9 chars; first 5 chars dropped → "html".
        let out = layout::scroll_line(&line, 5, 1);
        let text = line_text(&out);
        // Prefix is intact, then "html" (rest of primary), then separator,
        // then secondary.
        assert!(text.starts_with("★ "));
        assert!(text.contains("html"));
        assert!(!text.contains("text/")); // the dropped portion is gone
        assert!(text.contains("HTML document"));
    }

    #[test]
    fn hscroll_can_remove_entire_spans() {
        let line = sample_row();
        // "text/html" (9) + "    " (4) = 13 — skip all of those, land at "HTML…".
        let out = layout::scroll_line(&line, 13, 1);
        let text = line_text(&out);
        assert!(text.starts_with("★ "));
        assert!(text.contains("HTML document"));
        assert!(!text.contains("text/html"));
    }

    #[test]
    fn hscroll_past_everything_leaves_only_prefix() {
        let line = sample_row();
        let out = layout::scroll_line(&line, 9999, 1);
        assert_eq!(line_text(&out), "★ ");
    }

    #[test]
    fn span_style_is_preserved_after_partial_skip() {
        let style = Style::default().add_modifier(Modifier::BOLD);
        let line = Line::from(vec![
            Span::raw("p "),                          // prefix
            Span::styled("abcdef", style),            // primary, bolded
        ]);
        let out = layout::scroll_line(&line, 3, 1);
        // We skipped 3 chars of "abcdef" → "def"; style must follow.
        let trimmed_span = &out.spans[1];
        assert_eq!(trimmed_span.content.as_ref(), "def");
        assert_eq!(trimmed_span.style, style);
    }
}

fn centered_rect(area: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}
