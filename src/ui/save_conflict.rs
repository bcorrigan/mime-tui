use crate::config::{MimeTuiConfig, Theme};
use crate::storage::mimeapps::{ConflictKind, MimeConflict};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

pub fn draw(f: &mut Frame, conflicts: &[MimeConflict], config: &MimeTuiConfig) {
    let n = conflicts.len();

    let border = Theme::parse_color(&config.colors.focus);
    let key = Style::default()
        .fg(Theme::parse_color(&config.colors.focus))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Theme::parse_color(&config.colors.secondary));
    let warn = Style::default()
        .fg(Theme::parse_color(&config.colors.highlight))
        .add_modifier(Modifier::BOLD);

    // Body: blurb + per-conflict lines + 4 action lines. Calculate so the
    // modal hugs its content on tall terminals.
    // Approx 5 lines blurb + n conflict lines (capped) + 6 actions = N rows.
    let max_show = 8usize;
    let shown = n.min(max_show);
    let body_height = (6 + shown + 6 + 2) as u16; // blurb + list + actions + padding
    let area = centered_rect(f.area(), 70, body_height);
    f.render_widget(Clear, area);

    let text = crate::ui::layout::theme_text_style(config);
    let block = Block::default()
        .title(" mimeapps.list changed externally ")
        .borders(Borders::ALL)
        .border_type(Theme::parse_border_type(&config.colors.border_style))
        .border_style(Style::default().fg(border))
        .style(text);

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "Another process modified ~/.config/mimeapps.list since you opened mime-tui.",
            dim,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!(
                "{} of your pending edit{} conflict{} with external change{}:",
                n,
                if n == 1 { "" } else { "s" },
                if n == 1 { "s" } else { "" },
                if n == 1 { "" } else { "s" },
            ),
            dim,
        ),
    ]));
    lines.push(Line::from(""));

    for c in conflicts.iter().take(max_show) {
        lines.push(conflict_line(c, warn, dim));
    }
    if n > max_show {
        lines.push(Line::from(Span::styled(
            format!("    … and {} more", n - max_show),
            dim,
        )));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("r", key),
        Span::styled("  reload from disk — discards your pending edits", dim),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("o", key),
        Span::styled("  overwrite — your edits win, external changes lost", dim),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("m", key),
        Span::styled(
            "  merge — drop the conflicting edits, save the rest",
            dim,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("c", key),
        Span::styled(" / ", dim),
        Span::styled("Esc", key),
        Span::styled("  cancel — leave pending in memory, no write", dim),
    ]));

    let para = Paragraph::new(lines)
        .block(block)
        .style(text)
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn conflict_line(c: &MimeConflict, warn: Style, dim: Style) -> Line<'static> {
    let detail = match &c.kind {
        ConflictKind::DefaultChanged { ours, theirs } => format!(
            "default: you set {}, disk now has {}",
            describe_opt(ours.as_deref()),
            describe_opt(theirs.as_deref()),
        ),
        ConflictKind::AddRemoveOpposed { app_id, we_added } => {
            if *we_added {
                format!("you added {}, disk now removes it", app_id)
            } else {
                format!("you removed {}, disk now adds it", app_id)
            }
        }
    };
    Line::from(vec![
        Span::raw("    "),
        Span::styled("•  ", warn),
        Span::styled(c.mime.clone(), warn),
        Span::raw("  "),
        Span::styled(detail, dim),
    ])
}

fn describe_opt(s: Option<&str>) -> String {
    match s {
        Some(s) => s.to_string(),
        None => "(cleared)".into(),
    }
}

fn centered_rect(area: Rect, percent_x: u16, fixed_height: u16) -> Rect {
    let total_h = area.height;
    let top = total_h.saturating_sub(fixed_height) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top),
            Constraint::Length(fixed_height.min(total_h)),
            Constraint::Min(0),
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
