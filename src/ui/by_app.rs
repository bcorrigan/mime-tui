use crate::app::{App, Focus, Relation};
use crate::config::{MimeTuiConfig, Theme};
use crate::icons;
use crate::ui::layout;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

/// Pick the per-marker foreground colour for the by-app right pane —
/// mirrors `picker::marker_style` so the same relation reads identically
/// in both places.
fn marker_style(rel: Relation, config: &MimeTuiConfig) -> Style {
    let colour_field = match rel {
        Relation::Default => &config.colors.marker_default,
        Relation::Associated => &config.colors.marker_associated,
        Relation::DeclaredOnly => &config.colors.marker_declared_only,
    };
    Style::default().fg(Theme::parse_color(colour_field))
}

pub fn draw(f: &mut Frame, app: &mut App, area: Rect, config: &MimeTuiConfig) {
    // App names are typically short and the right pane wants room to show
    // the mime list with markers; give the right side more space.
    let (left_area, right_area) = layout::horizontal_split(area, 35);

    let visible: Vec<crate::model::DesktopApp> =
        app.visible_apps().into_iter().cloned().collect();
    let display: Vec<String> = visible
        .iter()
        .map(|a| format!("{}  {}", icons::category_icon(&a.category), a.name))
        .collect();

    let max_content_w: usize = display.iter().map(|s| s.chars().count()).max().unwrap_or(0);
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

    let clamped_left = app.selected_left.min(scrolled.len().saturating_sub(1));
    let focus_left = app.focus == Focus::Left;

    layout::render_list(
        f,
        left_area,
        " Applications ",
        &scrolled,
        Some(clamped_left),
        focus_left,
        config,
        &mut app.left_list_state,
    );
    layout::render_list_hscrollbar(f, left_area, max_content_w, hscroll, config);
    app.left_rect = Some(left_area);

    let selected_app = visible.get(clamped_left).cloned();
    draw_right(f, app, right_area, selected_app.as_ref(), config);
    app.right_rect = Some(right_area);
}

fn draw_right(
    f: &mut Frame,
    app: &mut App,
    area: Rect,
    selected: Option<&crate::model::DesktopApp>,
    config: &MimeTuiConfig,
) {
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(6), Constraint::Min(3)])
        .split(area);

    render_summary(f, split[0], selected, config);
    render_mimes_list(f, split[1], selected, app, config);
}

fn render_summary(
    f: &mut Frame,
    area: Rect,
    selected: Option<&crate::model::DesktopApp>,
    config: &MimeTuiConfig,
) {
    let border_color = Theme::parse_color(&config.colors.border);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));

    let title = selected
        .map(|a| format!(" {} ", a.name))
        .unwrap_or_else(|| " — ".into());

    let text = layout::theme_text_style(config);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border_color))
        .style(text);

    let mut lines: Vec<Line> = Vec::new();
    if let Some(a) = selected {
        if !a.comment.is_empty() {
            lines.push(Line::from(a.comment.clone()));
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(format!("id: {}", a.id), dim)));
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .style(text)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn render_mimes_list(
    f: &mut Frame,
    area: Rect,
    selected: Option<&crate::model::DesktopApp>,
    app: &mut App,
    config: &MimeTuiConfig,
) {
    let Some(a) = selected else {
        layout::render_list(
            f,
            area,
            " MIME types ",
            &[],
            None,
            false,
            config,
            &mut app.right_list_state,
        );
        return;
    };

    let app_id = a.id.clone();
    let list = app.displayable_mime_list_for_app(&app_id);
    let secondary = Style::default().fg(Theme::parse_color(&config.colors.secondary));
    let pending_style = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);

    // Build each row as a Line so the relation marker can carry its own
    // themed colour. Layout per row:
    //
    //   span[0] = pending sigil ("*" or " " — styled by pending_style)
    //   span[1] = " "  (1-char separator)
    //   span[2] = relation marker (★ / ✓ / · — styled by marker_style)
    //   span[3] = " "  (1-char separator, raw)
    //   span[4] = mime id (bold when pending, strikethrough when removed)
    //   span[5] = " (default)" / " (declared, not associated)" (dim) if any
    //
    // pin_count = 4 keeps the [sigil + space + marker + space] prefix stuck
    // at the left edge when horizontally scrolled.
    let items: Vec<Line<'static>> = list
        .iter()
        .map(|(m, rel, is_removed)| {
            let is_pending = app.is_pending_row(&m.id, &app_id);
            let sigil = if is_pending { "*" } else { " " };
            // Strikethrough is the dominant cue for "going away" — drop
            // bold so the row reads as subdued, not emphasised.
            let id_style = if *is_removed {
                Style::default().add_modifier(Modifier::CROSSED_OUT)
            } else if is_pending {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let suffix_style = if *is_removed {
                secondary.add_modifier(Modifier::CROSSED_OUT)
            } else {
                secondary
            };
            let rel_char = match rel {
                Relation::Default => "★",
                Relation::Associated => "✓",
                Relation::DeclaredOnly => "·",
            };
            let suffix = match rel {
                Relation::Default => "  (default)",
                Relation::Associated => "",
                Relation::DeclaredOnly => "  (declared, not associated)",
            };
            let mut spans = vec![
                Span::styled(sigil.to_string(), pending_style),
                Span::raw(" "),
                Span::styled(rel_char.to_string(), marker_style(*rel, config)),
                Span::raw(" "),
                Span::styled(m.id.clone(), id_style),
            ];
            if !suffix.is_empty() {
                spans.push(Span::styled(suffix.to_string(), suffix_style));
            }
            Line::from(spans)
        })
        .collect();

    // Long mime ids overflow the pane — Shift+←/→ scrolls when focused,
    // scrollbar appears at the bottom only on overflow.
    let max_content_w: usize = items.iter().map(|l| l.width()).max().unwrap_or(0);
    let inner_w = (area.width as usize).saturating_sub(4);
    let hscroll = (app.right_hscroll as usize).min(max_content_w.saturating_sub(inner_w));
    app.right_hscroll = hscroll as u16;

    let scrolled: Vec<Line<'static>> = items
        .iter()
        .map(|l| layout::scroll_line(l, hscroll, 4))
        .collect();

    let focus_right = app.focus == Focus::Right;
    let clamped = app.selected_right.min(scrolled.len().saturating_sub(1));
    let selected = if focus_right { Some(clamped) } else { None };
    layout::render_list_lines(
        f,
        area,
        " MIME types ",
        &scrolled,
        selected,
        focus_right,
        config,
        &mut app.right_list_state,
    );
    layout::render_list_hscrollbar(f, area, max_content_w, hscroll, config);
}
