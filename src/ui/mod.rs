use crate::app::{App, Mode, View};
use crate::config::MimeTuiConfig;
use ratatui::Frame;

pub mod layout;
mod by_app;
mod by_mime;
mod confirm;
mod help;
mod picker;
mod save_conflict;
mod status;
mod theme_pick;

pub fn draw(f: &mut Frame, app: &mut App, config: &MimeTuiConfig) {
    let (search_area, rest) = layout::vertical_split(f, 3, config.search_position.clone());
    layout::render_search_bar(f, search_area, &app.input, app.focus, config);
    app.search_rect = Some(search_area);

    // Status height grows with content — wraps onto extra lines on narrow
    // terminals so no hints get clipped off the right edge.
    let status_height = status::required_height(app, config, rest.width);
    let (content_area, status_area) = layout::content_with_status_split(rest, status_height);

    match app.view {
        View::ByMime => by_mime::draw(f, app, content_area, config),
        View::ByApp => by_app::draw(f, app, content_area, config),
    }

    status::draw(f, app, status_area, config);

    // Overlays render on top, last.
    match app.mode.clone() {
        Mode::PickApp { .. } | Mode::PickMime { .. } => picker::draw(f, app, config),
        Mode::ConfirmQuit => confirm::draw(f, app, config),
        Mode::Help => help::draw(f, app, config),
        Mode::ConflictResolve { conflicts } => {
            save_conflict::draw(f, &conflicts, config)
        }
        Mode::ThemePick { selected, .. } => {
            theme_pick::draw(f, app, selected, config)
        }
        Mode::Browse => {}
    }
}
