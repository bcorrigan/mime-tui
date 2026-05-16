//! Pre-save review modal: shows the user every pending edit grouped by
//! mime, lets them scroll through, and routes Enter / y back to the real
//! save path in events::do_save.
//!
//! Layout uses `Paragraph::scroll((y, x))` — same primitive the help
//! overlay uses — paired with both the vertical and horizontal scrollbar
//! helpers so the modal feels identical to the main panes when content
//! overflows.

use crate::app::{App, ChangeSummary, DefaultChange};
use crate::config::{MimeTuiConfig, Theme};
use crate::ui::layout;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

pub fn draw(f: &mut Frame, app: &mut App, config: &MimeTuiConfig) {
    let summaries = app.pending_change_summary();

    // Size the modal generously — review content tends to scroll, and a
    // wide column keeps mime ids on one line whenever possible.
    let area = centered_rect(f.area(), 80, 80);
    f.render_widget(Clear, area);

    let border = Theme::parse_color(&config.colors.focus);
    let text = layout::theme_text_style(config);

    let title = format!(
        " Confirm save — {} pending change{} ",
        app.pending.count(),
        if app.pending.count() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border))
        .style(text);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner into [body | 1-line footer]. The footer renders the
    // accept/cancel hints right above the bottom border so they're always
    // in view even when the body scrolls.
    let layout_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let body_area = layout_chunks[0];
    let footer_area = layout_chunks[1];

    // Build the body lines. `inner_w_for_lines` is the renderable width
    // after the 1-col left padding we add to every row (cosmetic gutter
    // so content isn't flush against the border).
    let lines = build_lines(&summaries, app, config);
    let visual_rows = lines.len() as u16;
    let max_content_w = lines.iter().map(|l| l.width()).max().unwrap_or(0);

    // Clamp scroll offsets against actual content. End-key handlers pass
    // u16::MAX as a "go to bottom" sentinel; we resolve it here so all
    // scroll math lives in one place.
    let max_vscroll = visual_rows.saturating_sub(body_area.height);
    let vscroll = app.confirm_save_vscroll.min(max_vscroll);
    app.confirm_save_vscroll = vscroll;

    let max_hscroll = (max_content_w as u16).saturating_sub(body_area.width);
    let hscroll = app.confirm_save_hscroll.min(max_hscroll);
    app.confirm_save_hscroll = hscroll;

    f.render_widget(
        Paragraph::new(lines)
            .style(text)
            .scroll((vscroll, hscroll)),
        body_area,
    );

    // Both scrollbars use the same helpers as the main panes so they
    // visually match (same chars, same theme colours).
    layout::render_list_scrollbar(
        f,
        body_area,
        visual_rows as usize,
        vscroll as usize,
        true,
        config,
    );
    layout::render_list_hscrollbar(
        f,
        body_area,
        max_content_w,
        hscroll as usize,
        config,
    );

    // Footer hint — each atom is clickable via the status-click prelude.
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));
    let footer_chunks = vec![
        layout::HintChunk {
            spans: vec![Span::raw(" ")],
            action: None,
        },
        layout::HintChunk {
            spans: vec![
                Span::styled("Enter / y", key),
                Span::styled(": save  ", dim),
            ],
            action: Some(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        },
        layout::HintChunk {
            spans: vec![
                Span::styled("Esc / n", key),
                Span::styled(": cancel  ", dim),
            ],
            action: Some(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
        },
        layout::HintChunk {
            spans: vec![
                Span::styled("↑↓ PgUp PgDn", key),
                Span::styled(": scroll", dim),
            ],
            action: Some(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE)),
        },
    ];
    layout::render_hint_line(
        f,
        footer_area,
        footer_chunks,
        text,
        &mut app.status_clickables,
    );
}

fn build_lines(
    summaries: &[ChangeSummary],
    app: &App,
    config: &MimeTuiConfig,
) -> Vec<Line<'static>> {
    let header_style = Style::default()
        .fg(Theme::parse_color(&config.colors.highlight))
        .add_modifier(Modifier::BOLD);
    let default_style =
        Style::default().fg(Theme::parse_color(&config.colors.marker_default));
    let add_style =
        Style::default().fg(Theme::parse_color(&config.colors.marker_associated));
    let remove_style = Style::default()
        .fg(Theme::parse_color(&config.colors.secondary))
        .add_modifier(Modifier::CROSSED_OUT);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(Line::from(""));

    if summaries.is_empty() {
        // Defensive: open_confirm_save is gated on is_dirty(), but render
        // an explicit empty-state in case state changes between opening
        // the modal and drawing it.
        lines.push(Line::from(Span::styled(
            "  No pending edits.".to_string(),
            dim,
        )));
        return lines;
    }

    for (i, cs) in summaries.iter().enumerate() {
        if i > 0 {
            // Spacer between mime sections so the eye groups each block.
            lines.push(Line::from(""));
        }

        // Section header: mime id, bold + themed colour.
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(cs.mime.clone(), header_style),
        ]));

        if let Some(dc) = &cs.default_change {
            push_default_lines(&mut lines, dc, app, default_style, dim);
        }

        for add_id in &cs.adds {
            let name = app.app_name_for(add_id);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("+ ", add_style),
                Span::raw(name),
                Span::styled(format!("    {}", add_id), dim),
            ]));
        }

        for rem_id in &cs.removes {
            let name = app.app_name_for(rem_id);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("− ", remove_style),
                Span::styled(name, remove_style),
                Span::styled(format!("    {}", rem_id), remove_style),
            ]));
        }
    }
    lines.push(Line::from(""));
    lines
}

fn push_default_lines(
    lines: &mut Vec<Line<'static>>,
    dc: &DefaultChange,
    app: &App,
    default_style: Style,
    dim: Style,
) {
    match dc {
        DefaultChange::Set { new, old } => {
            let new_name = app.app_name_for(new);
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("★ ", default_style),
                Span::raw("default → "),
                Span::raw(new_name),
                Span::styled(format!("    {}", new), dim),
            ]));
            if let Some(old_id) = old {
                let old_name = app.app_name_for(old_id);
                lines.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(
                        format!("(was: {}    {})", old_name, old_id),
                        dim,
                    ),
                ]));
            }
        }
        DefaultChange::Cleared { old } => {
            lines.push(Line::from(vec![
                Span::raw("    "),
                Span::styled("★ ", default_style),
                Span::raw("default cleared"),
            ]));
            if let Some(old_id) = old {
                let old_name = app.app_name_for(old_id);
                lines.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(
                        format!("(was: {}    {})", old_name, old_id),
                        dim,
                    ),
                ]));
            }
        }
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
