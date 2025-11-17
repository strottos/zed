use std::{
    io,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossterm::{
    event::{self, Event as CEvent, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    text::Span,
    widgets::{Block, Borders, Paragraph},
};

/// Run a minimal Ratatui-based TUI.
///
/// This function owns terminal initialization and teardown so callers can be
/// simple: parse CLI in `main.rs` and then call `app::run_app(&args)`.
pub fn run_app() -> Result<()> {
    // Initialize terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Ensure we always restore terminal state on return.
    // We capture the result of the run and perform cleanup afterwards.
    let run_result = (|| -> Result<()> {
        // App state
        let mut counter: usize = 0;
        let tick_rate = Duration::from_millis(200);
        let mut last_tick = Instant::now();

        loop {
            terminal.draw(|f| {
                let size = f.area();

                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([Constraint::Length(3), Constraint::Min(1)].as_ref())
                    .split(size);

                let header = Paragraph::new(Span::raw(format!(
                    "zed-tui  — {}",
                    env!("CARGO_PKG_VERSION")
                )))
                .block(Block::default().borders(Borders::ALL).title("Header"));

                f.render_widget(header, chunks[0]);

                let body_text = "Stuff and things.".to_string();
                let body = Paragraph::new(body_text).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Files / Status"),
                );

                f.render_widget(body, chunks[1]);

                // Small footer showing an interactive counter
                let footer = Paragraph::new(Span::raw(format!(
                    "Press '+' or '-' to change counter. Counter: {}",
                    counter
                )))
                .block(Block::default().borders(Borders::ALL).title("Controls"));
                // Render footer overlay at bottom
                let footer_area = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(3)].as_ref())
                    .split(chunks[1])[1];
                f.render_widget(footer, footer_area);
            })?;

            // compute time until next tick
            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_secs(0));

            // Poll for events with timeout so we redraw periodically
            if event::poll(timeout)? {
                match event::read()? {
                    CEvent::Key(key) => match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => break,
                        KeyCode::Char('+') | KeyCode::Char('=') => {
                            counter = counter.saturating_add(1);
                        }
                        KeyCode::Char('-') => {
                            counter = counter.saturating_sub(1);
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
            }
        }

        Ok(())
    })();

    // Teardown / cleanup terminal state
    // We intentionally ignore errors during cleanup so we return the original run_result error if any.
    let _ = disable_raw_mode();
    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = terminal.show_cursor();

    run_result
}
