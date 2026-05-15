use crate::app::{App, Focus};
use crate::config::{MimeTuiConfig, Theme};
use crate::icons;
use crate::storage;
use crate::ui::layout;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

/// If the default for `mime_id` was read from a file other than the user's own
/// `mimeapps.list`, return that file's basename for display. Otherwise None
/// (meaning the default lives where saves go, so no warning is needed).
fn source_label(app: &App, mime_id: &str) -> Option<String> {
    let src = app.assoc.default_sources.get(mime_id)?;
    let user = storage::mimeapps::user_mimeapps_path()?;
    if src == &user {
        return None;
    }
    Some(
        src.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| src.to_string_lossy().into_owned()),
    )
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect, config: &MimeTuiConfig) {
    // Mime ids can be very long (e.g. "application/vnd.oasis.opendocument…"),
    // so give the left list 60% of the width.
    let (left_area, right_area) = layout::horizontal_split(area, 60);

    let visible: Vec<crate::model::MimeType> =
        app.visible_mimes().into_iter().cloned().collect();
    let display: Vec<String> = visible
        .iter()
        .map(|m| format!("{}  {}", icons::mime_icon(&m.id), m.id))
        .collect();

    // Compute max content width for hscroll/scrollbar — long mime ids spill
    // off the right edge of the pane on smaller terminals.
    let max_content_w: usize = display.iter().map(|s| s.chars().count()).max().unwrap_or(0);
    // `render_list` adds " {item} " padding (1 col each side) inside a
    // bordered block — so usable content width = area.width - 4.
    let inner_w = (left_area.width as usize).saturating_sub(4);
    let hscroll = (app.left_hscroll as usize).min(max_content_w.saturating_sub(inner_w));
    app.left_hscroll = hscroll as u16;

    let scrolled: Vec<String> = if hscroll == 0 {
        display.clone()
    } else {
        display
            .iter()
            .map(|s| s.chars().skip(hscroll).collect::<String>())
            .collect()
    };

    // Style each row: a mime whose default / added / pending assocs reach
    // a non-installed `.desktop` gets the `invalid` colour + bold, so the
    // user can scan the list and immediately see which mimes have stale
    // entries to clean up.
    let invalid_style = Style::default()
        .fg(Theme::parse_color(&config.colors.invalid))
        .add_modifier(Modifier::BOLD);
    let items: Vec<Line<'static>> = scrolled
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let mime = &visible[i];
            let style = if app.mime_has_missing(&mime.id) {
                invalid_style
            } else {
                Style::default()
            };
            Line::from(Span::styled(s.clone(), style))
        })
        .collect();

    let clamped_left = app.selected_left.min(items.len().saturating_sub(1));
    let focus_left = app.focus == Focus::Left;

    layout::render_list_lines(
        f,
        left_area,
        " MIME types ",
        &items,
        Some(clamped_left),
        focus_left,
        config,
        &mut app.left_list_state,
    );
    layout::render_list_hscrollbar(f, left_area, max_content_w, hscroll, config);
    app.left_rect = Some(left_area);

    let selected_mime = visible.get(clamped_left).cloned();
    draw_right(f, app, right_area, selected_mime.as_ref(), config);
    app.right_rect = Some(right_area);
}

fn draw_right(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    mime: Option<&crate::model::MimeType>,
    config: &MimeTuiConfig,
) {
    // Top: description + default (paragraph). Bottom: selectable associations.
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(3)])
        .split(area);

    render_summary(f, split[0], mime, app, config);
    render_associations(f, split[1], mime, app, config);
}

fn render_summary(
    f: &mut Frame,
    area: Rect,
    mime: Option<&crate::model::MimeType>,
    app: &App,
    config: &MimeTuiConfig,
) {
    let border_color = Theme::parse_color(&config.colors.border);
    let heading = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let title = mime
        .map(|m| format!(" {} ", m.id))
        .unwrap_or_else(|| " — ".into());

    let text = layout::theme_text_style(config);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(m) = mime {
        let desc = if m.description.is_empty() {
            "(no description)".into()
        } else {
            m.description.clone()
        };
        lines.push(Line::from(desc));
        lines.push(Line::from(""));

        let pending_marker = if app.pending.set_default.contains_key(&m.id) {
            Span::styled(" *", heading)
        } else {
            Span::raw("")
        };
        let header = Line::from(vec![
            Span::styled("Default", heading),
            pending_marker,
        ]);
        lines.push(header);
        match app.effective_default_for(&m.id) {
            Some(d) => {
                lines.push(Line::from(format!("  {}", d.name)));
                lines.push(Line::from(Span::styled(format!("  {}", d.id), dim)));
                if let Some(src) = source_label(app, &m.id) {
                    lines.push(Line::from(Span::styled(
                        format!("  ⚠ from {} (saves will be shadowed)", src),
                        dim,
                    )));
                }
            }
            None => match app.missing_default_for(&m.id) {
                Some(missing_id) => {
                    let invalid = Style::default()
                        .fg(Theme::parse_color(&config.colors.invalid))
                        .add_modifier(Modifier::BOLD);
                    lines.push(Line::from(Span::styled(
                        format!("  {}", missing_id),
                        invalid,
                    )));
                    lines.push(Line::from(Span::styled(
                        "  (app not installed)".to_string(),
                        invalid,
                    )));
                }
                None => lines.push(Line::from(Span::styled("  (none set)", dim))),
            },
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(text)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_associations(
    f: &mut Frame,
    area: Rect,
    mime: Option<&crate::model::MimeType>,
    app: &mut App,
    config: &MimeTuiConfig,
) {
    let Some(m) = mime else {
        layout::render_list(
            f,
            area,
            " Associations ",
            &[],
            None,
            false,
            config,
            &mut app.right_list_state,
        );
        return;
    };
    let mime_id = m.id.clone();
    let assoc = app.displayable_associations_for(&mime_id);

    // For the "(default)" suffix we want the *would-be* default — the app
    // that holds the star when ignoring pending.remove. `effective_default_for`
    // returns None when the current default is pending-removed, which would
    // hide the marker; we want to keep it so the user sees what they're
    // removing.
    let would_be_default_id: Option<String> = if let Some(slot) =
        app.pending.set_default.get(&mime_id)
    {
        // A pending default-change wins.
        slot.clone()
    } else {
        app.assoc.defaults.get(&mime_id).cloned()
    };
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));
    let pending_style = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);

    // Each row: [pending-sigil][space][name][optional "  (default)" suffix].
    // pin_count = 2 so the sigil + its trailing space stay anchored at the
    // left edge when the row is horizontally scrolled.
    let mut items: Vec<Line<'static>> = assoc
        .iter()
        .map(|(e, is_removed)| {
            let is_pending = app.is_pending_row(&mime_id, &e.id);
            let is_default = Some(&e.id) == would_be_default_id.as_ref();
            let sigil = if is_pending { "*" } else { " " };
            // Strikethrough wins as the dominant visual cue for removals —
            // we drop the bold to keep the row looking subdued ("going
            // away") rather than emphasised ("look at me").
            let name_style = if *is_removed {
                Style::default().add_modifier(Modifier::CROSSED_OUT)
            } else if is_pending {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let suffix_style = if *is_removed {
                dim.add_modifier(Modifier::CROSSED_OUT)
            } else {
                dim
            };
            let mut spans = vec![
                Span::styled(sigil.to_string(), pending_style),
                Span::raw(" "),
                Span::styled(e.name.clone(), name_style),
            ];
            if is_default {
                spans.push(Span::styled("  (default)".to_string(), suffix_style));
            }
            Line::from(spans)
        })
        .collect();

    // Append phantom rows — entries on disk / pending that point at a
    // `.desktop` we don't see in the installed apps index. Rendered bold
    // in the `invalid` colour and tagged "(app not installed)" so the
    // user can spot — and `r` to clean up — stale mimeapps.list entries
    // left behind by uninstalled apps.
    let invalid_style = Style::default()
        .fg(Theme::parse_color(&config.colors.invalid))
        .add_modifier(Modifier::BOLD);
    let invalid_struck = invalid_style.add_modifier(Modifier::CROSSED_OUT);
    for missing in app.missing_associations_for(&mime_id) {
        let row_style = if missing.is_pending_removed {
            invalid_struck
        } else {
            invalid_style
        };
        let is_pending = app.is_pending_row(&mime_id, &missing.app_id);
        let sigil = if is_pending { "*" } else { " " };
        let mut spans = vec![
            Span::styled(sigil.to_string(), pending_style),
            Span::raw(" "),
            Span::styled(missing.app_id.clone(), row_style),
        ];
        if missing.is_default {
            spans.push(Span::styled("  (default)".to_string(), row_style));
        }
        spans.push(Span::styled("  (app not installed)".to_string(), row_style));
        items.push(Line::from(spans));
    }

    // Associations are usually app names (short), but a verbose
    // "(default)" suffix can still overflow on narrow panes.
    let max_content_w: usize = items.iter().map(|l| l.width()).max().unwrap_or(0);
    let inner_w = (area.width as usize).saturating_sub(4);
    let hscroll = (app.right_hscroll as usize).min(max_content_w.saturating_sub(inner_w));
    app.right_hscroll = hscroll as u16;

    let scrolled: Vec<Line<'static>> = items
        .iter()
        .map(|l| layout::scroll_line(l, hscroll, 2))
        .collect();

    let focus_right = app.focus == Focus::Right;
    let clamped = app.selected_right.min(scrolled.len().saturating_sub(1));
    // Only show the selection bar when the right pane is focused. With focus
    // on the left, the right pane is just informational — a "ghost" selection
    // here would imply navigability that isn't actually active.
    let selected = if focus_right { Some(clamped) } else { None };
    layout::render_list_lines(
        f,
        area,
        " Associations ",
        &scrolled,
        selected,
        focus_right,
        config,
        &mut app.right_list_state,
    );
    layout::render_list_hscrollbar(f, area, max_content_w, hscroll, config);
}
