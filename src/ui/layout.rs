use ratatui::{
    Frame,
    layout::{Layout, Constraint, Direction, Rect},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph,
        Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
    style::Style,
};
use tui_input::Input;
use crate::app::Focus;
use crate::config::{MimeTuiConfig, Theme, SearchPosition};

/// Base text style for the current theme. Apply to Block.style /
/// Paragraph.style / List.style so plain (`Span::raw`) text picks up the
/// configured foreground; styled spans override per-span.
pub fn theme_text_style(config: &MimeTuiConfig) -> Style {
    Style::default().fg(Theme::parse_color(&config.colors.text))
}

pub fn vertical_split(f: &Frame, search_height: u16, search_position: SearchPosition) -> (Rect, Rect) {
    let full_area = f.area();
    match search_position {
        SearchPosition::Top => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(search_height), Constraint::Min(0)])
                .split(full_area);
            (chunks[0], chunks[1])
        }
        SearchPosition::Bottom => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(0), Constraint::Length(search_height)])
                .split(full_area);
            (chunks[1], chunks[0])
        }
    }
}

/// Top region for the content panes, bottom strip for the status bar. The
/// bottom strip grows to `status_height` rows so the status hints can wrap
/// onto multiple lines when the terminal is too narrow to fit them in one.
pub fn content_with_status_split(area: Rect, status_height: u16) -> (Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(status_height.max(1))])
        .split(area);
    (chunks[0], chunks[1])
}

/// Split a content area into left/right panes with `left_pct` of the width
/// going to the left pane. The two views use different splits — mime ids are
/// long, so by-mime gets more room on the left; by-app gives the right pane
/// (mime relations) more space.
pub fn horizontal_split(area: Rect, left_pct: u16) -> (Rect, Rect) {
    let left_pct = left_pct.clamp(10, 90);
    let right_pct = 100 - left_pct;
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(left_pct), Constraint::Percentage(right_pct)])
        .split(area);
    (chunks[0], chunks[1])
}

#[allow(dead_code)] // Reserved for help/about overlay (Phase 4 polish).
pub fn render_description(
    f: &mut Frame,
    area: Rect,
    title: &str,
    body: &str,
    config: &MimeTuiConfig,
) {
    let border_color = Theme::parse_color(&config.colors.border);
    let text_style = theme_text_style(config);
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text_style);
    let paragraph = Paragraph::new(body)
        .block(block)
        .style(text_style)
        .wrap(Wrap { trim: true });
    f.render_widget(paragraph, area);
}

pub fn render_search_bar(
    f: &mut Frame,
    area: Rect,
    input: &Input,
    focus: Focus,
    config: &MimeTuiConfig,
) {
    let border_color = if focus == Focus::Search {
        Theme::parse_color(&config.colors.focus)
    } else {
        Theme::parse_color(&config.colors.border)
    };

    let text = theme_text_style(config);
    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text);

    let inner = block.inner(area);

    let query = input.value();
    let cursor_position = input.cursor();

    let query_chars: Vec<char> = query.chars().collect();
    let query_len = query_chars.len();

    let padding = 1;
    let available_width = (inner.width as usize).saturating_sub(padding * 2);

    let scroll_offset = if cursor_position >= available_width {
        cursor_position - available_width + 1
    } else {
        0
    };

    let visible_start = scroll_offset;
    let visible_end = (visible_start + available_width).min(query_len);
    let visible_text: String = query_chars[visible_start..visible_end].iter().collect();

    let padded_text = format!(" {} ", visible_text);

    let paragraph = Paragraph::new(padded_text).block(block).style(text);

    f.render_widget(paragraph, area);

    let cursor_x = inner.x + padding as u16 + (cursor_position - scroll_offset) as u16;
    let cursor_y = inner.y;
    f.set_cursor_position((cursor_x, cursor_y));
}

/// `selected: Some(i)` shows a highlight bar on row `i`; `None` renders the
/// list without any selection bar — used by the right pane when focus is on
/// the left, where a "ghost" selection isn't informative.
pub fn render_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: &[String],
    selected: Option<usize>,
    focus_on_title: bool,
    config: &MimeTuiConfig,
    state: &mut ListState,
) {
    match selected {
        Some(s) => {
            let sel = if s >= items.len() { 0 } else { s };
            state.select(Some(sel));
        }
        None => state.select(None),
    }

    let border_color = if focus_on_title {
        Theme::parse_color(&config.colors.focus)
    } else {
        Theme::parse_color(&config.colors.border)
    };
    let text = theme_text_style(config);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text);

    let list_items: Vec<ListItem> = items.iter()
        .map(|a| ListItem::new(format!(" {} ", a)))
        .collect();

    let selection_color = if focus_on_title {
        Theme::parse_color(&config.colors.highlight)
    } else {
        Theme::parse_color(&config.colors.unfocused)
    };

    let highlight_style = match config.colors.highlight_type.to_lowercase().as_str() {
        "foreground" => Style::default().fg(selection_color),
        "background" | _ => Style::default()
            .bg(selection_color)
            .fg(Theme::parse_color(&config.colors.selection_fg)),
    };

    let list = List::new(list_items)
        .block(block)
        .style(text)
        .highlight_style(highlight_style)
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);

    // After the List has rendered, `state.offset()` reflects the post-update
    // scroll position; pass that to the scrollbar so the thumb matches what
    // the user actually sees.
    render_list_scrollbar(f, area, items.len(), state.offset(), focus_on_title, config);
}

/// Render a vertical scrollbar on the right edge of `area` *only when* the
/// list content overflows the visible viewport. The scrollbar overlays the
/// block's right border, replacing it with a track + thumb.
///
/// Colours are read from `theme.scrollbar_thumb` and `theme.scrollbar_track`
/// (both fall back to `focus` / `unfocused` if left blank in the user's
/// config — see `config::resolve_fallback_colors`).
///
/// The `_focused` parameter is kept in the signature for callers that may
/// later want focus-aware tinting; today the colour is the user's themed
/// value regardless, since the border-colour change already conveys focus.
pub fn render_list_scrollbar(
    f: &mut Frame,
    area: Rect,
    content_length: usize,
    offset: usize,
    _focused: bool,
    config: &MimeTuiConfig,
) {
    // Inner viewport height = area minus top/bottom borders.
    let viewport = area.height.saturating_sub(2) as usize;
    if viewport == 0 || content_length <= viewport {
        return;
    }

    let thumb_fg = Theme::parse_color(&config.colors.scrollbar_thumb);
    let track_fg = Theme::parse_color(&config.colors.scrollbar_track);

    let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
        // No top/bottom caret arrows — they can compete with the block's
        // border corners. The track + thumb already communicate range.
        .begin_symbol(None)
        .end_symbol(None)
        .thumb_style(Style::default().fg(thumb_fg))
        .track_style(Style::default().fg(track_fg));

    // ratatui's Scrollbar treats `position` as if it ranged over
    // [0, content_length - 1] (i.e. a cursor index), while we feed it the
    // *scroll offset* which only ranges over [0, content_length - viewport].
    // Tell it our actual scroll-range size so the thumb reaches the end of
    // the track when the user is fully scrolled. See `part_lengths` in
    // ratatui-widgets/src/scrollbar.rs for the underlying formula.
    let max_offset = content_length - viewport;
    let sb_content_length = max_offset + 1;

    let mut sb_state = ScrollbarState::new(sb_content_length)
        .position(offset)
        .viewport_content_length(viewport);

    f.render_stateful_widget(scrollbar, area, &mut sb_state);
}

/// Horizontal sibling of [`render_list_scrollbar`]. Renders a scrollbar
/// along the **bottom** edge of `area` *only when* the content extends past
/// the visible width.
///
/// `content_width` is the widest item's display width; `offset` is how many
/// columns are scrolled off to the left.
pub fn render_list_hscrollbar(
    f: &mut Frame,
    area: Rect,
    content_width: usize,
    offset: usize,
    config: &MimeTuiConfig,
) {
    let viewport = area.width.saturating_sub(2) as usize;
    if viewport == 0 || content_width <= viewport {
        return;
    }

    let thumb_fg = Theme::parse_color(&config.colors.scrollbar_thumb);
    let track_fg = Theme::parse_color(&config.colors.scrollbar_track);

    let scrollbar = Scrollbar::new(ScrollbarOrientation::HorizontalBottom)
        .begin_symbol(None)
        .end_symbol(None)
        // ratatui's default thumb is `█` (FULL BLOCK) which fills an entire
        // cell — fine vertically (terminal cells are taller than wide, so it
        // reads thin) but visually chunky horizontally. Replace it with the
        // heavy box-drawing line: same height/baseline as the `─` track, just
        // bolder to stand out.
        .thumb_symbol("━")
        .thumb_style(Style::default().fg(thumb_fg))
        .track_style(Style::default().fg(track_fg));

    // Same adjustment as the vertical helper — see comment there. Without
    // this, the thumb stops short of the right edge even when there's no
    // more content to the right.
    let max_offset = content_width - viewport;
    let sb_content_length = max_offset + 1;

    let mut sb_state = ScrollbarState::new(sb_content_length)
        .position(offset)
        .viewport_content_length(viewport);

    f.render_stateful_widget(scrollbar, area, &mut sb_state);
}

/// Drop the first `hscroll` columns from `line`, but keep the first
/// `pin_count` spans intact at column 0 so contextual prefixes (e.g.
/// relation markers, mark indicators) stay visible while scrolling.
/// Char-width is approximated as 1 per char — acceptable for mime ids and
/// app names, which are almost always ASCII.
pub fn scroll_line(
    line: &Line<'static>,
    hscroll: usize,
    pin_count: usize,
) -> Line<'static> {
    if hscroll == 0 || line.spans.is_empty() {
        return line.clone();
    }
    let pin_count = pin_count.min(line.spans.len());
    let mut new_spans: Vec<Span<'static>> =
        line.spans.iter().take(pin_count).cloned().collect();
    let rest = &line.spans[pin_count..];

    let mut consumed: usize = 0;
    for span in rest {
        let span_w = span.width();
        if consumed + span_w <= hscroll {
            // Entire span lies before the scroll point — skip.
            consumed += span_w;
            continue;
        }
        if consumed >= hscroll {
            // Span is fully visible — include as-is.
            new_spans.push(span.clone());
        } else {
            // Span straddles the scroll boundary; keep its tail.
            let skip_chars = hscroll - consumed;
            let tail: String = span.content.as_ref().chars().skip(skip_chars).collect();
            new_spans.push(Span::styled(tail, span.style));
            consumed += span_w;
        }
    }

    Line::from(new_spans)
}

/// Same as `render_list` but accepts pre-styled `Line` items — used by the
/// picker so each row can carry mixed-style spans (e.g. a primary app name
/// followed by a dimmed `.desktop` id).
pub fn render_list_lines(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: &[Line<'static>],
    selected: Option<usize>,
    focus_on_title: bool,
    config: &MimeTuiConfig,
    state: &mut ListState,
) {
    match selected {
        Some(s) => {
            let sel = if s >= items.len() { 0 } else { s };
            state.select(Some(sel));
        }
        None => state.select(None),
    }

    let border_color = if focus_on_title {
        Theme::parse_color(&config.colors.focus)
    } else {
        Theme::parse_color(&config.colors.border)
    };
    let text = theme_text_style(config);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text);

    let list_items: Vec<ListItem> = items
        .iter()
        .map(|line| {
            // Match render_list's " {content} " padding so the two helpers
            // produce visually consistent rows.
            let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 2);
            spans.push(Span::raw(" "));
            spans.extend(line.spans.iter().cloned());
            spans.push(Span::raw(" "));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let selection_color = if focus_on_title {
        Theme::parse_color(&config.colors.highlight)
    } else {
        Theme::parse_color(&config.colors.unfocused)
    };

    let highlight_style = match config.colors.highlight_type.to_lowercase().as_str() {
        "foreground" => Style::default().fg(selection_color),
        "background" | _ => Style::default()
            .bg(selection_color)
            .fg(Theme::parse_color(&config.colors.selection_fg)),
    };

    let list = List::new(list_items)
        .block(block)
        .style(text)
        .highlight_style(highlight_style)
        .highlight_symbol("");

    f.render_stateful_widget(list, area, state);
    render_list_scrollbar(f, area, items.len(), state.offset(), focus_on_title, config);
}
