mod app;
mod config;
mod events;
mod icons;
mod model;
mod storage;
mod ui;

use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use crossterm::{
    ExecutableCommand,
    cursor::SetCursorStyle,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use eyre::Result;
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
};

use crate::app::App;
use crate::config::{CursorShape, load_config};

fn main() -> Result<()> {
    color_eyre::install()?;

    let cfg = load_config();
    let mut app = App::new(cfg)?;

    enable_raw_mode()?;
    let res = run_with_writer(io::stdout(), &mut app);
    disable_raw_mode()?;

    if let Err(err) = res {
        eprintln!("Error: {err:?}");
    }
    Ok(())
}

fn run_with_writer<W: Write + ExecutableCommand>(mut writer: W, app: &mut App) -> Result<()> {
    // Capture the cursor-color choice for symmetric handling on exit:
    // if we didn't set it on entry (empty / non-hex), we don't reset on
    // exit either, so the user's terminal cursor styling is left alone.
    let cursor_color = app.config.colors.cursor_color.clone();
    set_cursor_color(&mut writer, &cursor_color)?;

    execute!(writer, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(writer);
    let mut terminal = Terminal::new(backend)?;

    let res = run_app(&mut terminal, app, &cursor_color);

    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    reset_cursor_color(terminal.backend_mut(), &cursor_color)?;

    res
}

/// Tear down the TUI, raise SIGTSTP, then re-arm the TUI on resume.
///
/// This is the Ctrl-Z handler. raw mode + alt screen + mouse capture leave
/// stdin in a state the user's shell can't drive, so we must undo all of
/// that *before* suspending — otherwise the shell prompt would render into
/// our alternate buffer with raw-mode echo off. `libc::raise(SIGTSTP)`
/// blocks until the shell sends SIGCONT (typically via `fg`), at which
/// point we reinstate everything and force a redraw.
fn suspend<B: Backend + ExecutableCommand + Write>(
    terminal: &mut Terminal<B>,
    cursor_color: &str,
) -> Result<()>
where
    <B as Backend>::Error: Send + Sync + 'static,
{
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    reset_cursor_color(terminal.backend_mut(), cursor_color)?;
    disable_raw_mode()?;

    // SAFETY: raise is async-signal-safe and we're single-threaded.
    unsafe {
        libc::raise(libc::SIGTSTP);
    }

    enable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        EnterAlternateScreen,
        EnableMouseCapture
    )?;
    set_cursor_color(terminal.backend_mut(), cursor_color)?;
    // Force a full repaint: the terminal may have scrolled / been resized
    // / had foreground output written while we were stopped.
    terminal.clear()?;
    Ok(())
}

fn run_app<B: Backend + ExecutableCommand + Write>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    cursor_color: &str,
) -> Result<()>
where
    <B as Backend>::Error: Send + Sync + 'static,
{
    let mut last_input = Instant::now();

    loop {
        app.update_cursor_blink();

        let cfg = app.config.clone();
        terminal.draw(|f| ui::draw(f, app, &cfg))?;

        let style = if cfg.colors.cursor_blink_interval > 0 {
            match cfg.colors.cursor_shape {
                CursorShape::Block => SetCursorStyle::SteadyBlock,
                CursorShape::Underline => SetCursorStyle::SteadyUnderScore,
                CursorShape::Pipe => SetCursorStyle::SteadyBar,
            }
        } else {
            match cfg.colors.cursor_shape {
                CursorShape::Block => SetCursorStyle::BlinkingBlock,
                CursorShape::Underline => SetCursorStyle::BlinkingUnderScore,
                CursorShape::Pipe => SetCursorStyle::BlinkingBar,
            }
        };
        terminal.backend_mut().execute(style)?;

        if cfg.colors.cursor_blink_interval > 0 {
            if app.cursor_visible {
                terminal.show_cursor()?;
            } else {
                terminal.hide_cursor()?;
            }
        } else {
            terminal.show_cursor()?;
        }

        let tick = Duration::from_millis(50);

        if cfg.timeout > 0 && last_input.elapsed().as_secs() >= cfg.timeout {
            break;
        }

        if event::poll(tick)? {
            match event::read()? {
                Event::Key(key) => {
                    last_input = Instant::now();
                    if events::handle_key(app, key)? {
                        break;
                    }
                }
                Event::Mouse(mouse) => {
                    last_input = Instant::now();
                    if events::handle_mouse(app, mouse)? {
                        break;
                    }
                }
                _ => {}
            }
        }

        if app.pending_suspend {
            app.pending_suspend = false;
            suspend(terminal, cursor_color)?;
            // Don't let the idle-timeout fire immediately on resume just
            // because we were stopped for a long time.
            last_input = Instant::now();
        }
    }

    Ok(())
}

fn set_cursor_color<W: Write>(writer: &mut W, color_hex: &str) -> Result<()> {
    if let Some((r, g, b)) = parse_hex_color(color_hex) {
        write!(writer, "\x1b]12;rgb:{:02x}/{:02x}/{:02x}\x07", r, g, b)?;
        writer.flush()?;
    }
    Ok(())
}

fn reset_cursor_color<W: Write>(writer: &mut W, color_hex: &str) -> Result<()> {
    // Only reset if we actually set the cursor on entry. parse_hex_color
    // returning Some implies set_cursor_color emitted OSC 12, so we
    // balance with OSC 112 here. Empty / non-hex means we left the
    // terminal cursor alone — nothing to reset.
    if parse_hex_color(color_hex).is_some() {
        write!(writer, "\x1b]112\x07")?;
        writer.flush()?;
    }
    Ok(())
}

fn parse_hex_color(color: &str) -> Option<(u8, u8, u8)> {
    let color = color.trim();
    if !color.starts_with('#') {
        return None;
    }
    let hex = &color[1..];
    match hex.len() {
        3 => {
            let r = u8::from_str_radix(&hex[0..1], 16).ok()?;
            let g = u8::from_str_radix(&hex[1..2], 16).ok()?;
            let b = u8::from_str_radix(&hex[2..3], 16).ok()?;
            Some((r * 17, g * 17, b * 17))
        }
        6 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        8 => {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            Some((r, g, b))
        }
        _ => None,
    }
}
