use kode_tui::terminal::TerminalSession;
use ratatui::{buffer::Buffer, layout::Rect, widgets::Widget};
use std::{
    io,
    time::{Duration, Instant},
};

struct ProbeWidget;
impl Widget for ProbeWidget {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        if !area.is_empty() {
            buffer.set_stringn(
                area.x,
                area.y,
                "terminal probe",
                area.width as usize,
                ratatui::style::Style::default(),
            );
        }
    }
}

fn probe(mode: &str) -> io::Result<Option<String>> {
    let mut terminal = TerminalSession::open(None, mode == "enhanced")?;
    terminal.draw(|frame| frame.render_widget(ProbeWidget, frame.area()))?;
    match mode {
        "enhanced" => Ok(Some(format!("enhanced:{}", terminal.enhanced_keyboard()))),
        "panic" => panic!("deliberate terminal restoration probe"),
        "error" => Err(io::Error::other("deliberate terminal error")),
        "suspend" => {
            terminal.with_suspended(|| {
                assert!(!crossterm::terminal::is_raw_mode_enabled()?);
                #[cfg(unix)]
                {
                    let status = std::process::Command::new("/bin/sh")
                        .args(["-c", "read value; printf 'shell:%s\\n' \"$value\""])
                        .status()?;
                    assert!(status.success());
                }
                Ok(())
            })?;
            assert!(crossterm::terminal::is_raw_mode_enabled()?);
            terminal.draw(|frame| frame.render_widget(ProbeWidget, frame.area()))?;
            Ok(Some("suspended-and-resumed".into()))
        }
        "input" => {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                if let Some(event) = terminal.poll(Duration::from_millis(10))? {
                    return Ok(Some(format!("{event:?}")));
                }
            }
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "input probe timed out",
            ))
        }
        _ => Ok(Some("normal-exit".into())),
    }
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "normal".into());
    match probe(&mode) {
        Ok(result) => {
            if let Some(result) = result {
                println!("{result}");
            }
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
