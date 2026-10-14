//! Sole ownership of terminal modes, input and interactive rendering.
use crate::input::terminal::{InputEvent, normalize};
use crossterm::{
    cursor::{Hide, Show},
    event::{
        self, DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{self, EnterAlternateScreen, LeaveAlternateScreen},
};
use kode_platform::{jobs::MonotonicClock, logging::FileLog};
use ratatui::{Frame, Terminal, backend::CrosstermBackend};
use std::{
    io::{self, Stdout},
    panic::{self, PanicHookInfo},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

static OWNED: AtomicBool = AtomicBool::new(false);
static MODES_ACTIVE: AtomicBool = AtomicBool::new(false);
static ENHANCED: AtomicBool = AtomicBool::new(false);
type PanicHook = dyn Fn(&PanicHookInfo<'_>) + Send + Sync + 'static;

fn restore_modes() -> io::Result<()> {
    let mut output = io::stdout();
    let enhanced = if ENHANCED.swap(false, Ordering::AcqRel) {
        execute!(output, PopKeyboardEnhancementFlags)
    } else {
        Ok(())
    };
    let modes = execute!(
        output,
        DisableBracketedPaste,
        DisableFocusChange,
        DisableMouseCapture,
        Show,
        LeaveAlternateScreen
    );
    let raw = terminal::disable_raw_mode();
    enhanced.and(modes).and(raw)
}

struct ModeGuard;
impl ModeGuard {
    fn acquire() -> io::Result<Self> {
        OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::other("terminal already owned by another session"))?;
        Ok(Self)
    }
    fn enter(&self, enhanced: bool) -> io::Result<()> {
        terminal::enable_raw_mode()?;
        MODES_ACTIVE.store(true, Ordering::Release);
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange,
            EnableBracketedPaste,
            Hide
        )?;
        if enhanced {
            // Track before the write so a partial write is still rolled back.
            ENHANCED.store(true, Ordering::Release);
            execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                        | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                )
            )?;
        }
        Ok(())
    }
    fn leave(&self) -> io::Result<()> {
        if MODES_ACTIVE.swap(false, Ordering::AcqRel) {
            restore_modes()
        } else {
            Ok(())
        }
    }
}
impl Drop for ModeGuard {
    fn drop(&mut self) {
        let _ = self.leave();
        OWNED.store(false, Ordering::Release);
    }
}

struct PanicGuard {
    previous: Arc<PanicHook>,
}
impl PanicGuard {
    fn install(log: Option<Arc<FileLog>>) -> Self {
        let previous: Arc<PanicHook> = panic::take_hook().into();
        let prior = previous.clone();
        let ui_thread = thread::current().id();
        panic::set_hook(Box::new(move |info| {
            if let Some(log) = &log {
                let _ = log.record(
                    &MonotonicClock::default(),
                    "error",
                    "panic",
                    &info.to_string(),
                    serde_json::json!({"ui_thread": thread::current().id() == ui_thread}),
                );
            }
            if thread::current().id() == ui_thread {
                if MODES_ACTIVE.swap(false, Ordering::AcqRel) {
                    let _ = restore_modes();
                }
                prior(info);
            }
            // Worker panics are surfaced by Scheduler's typed completion; never
            // let the default hook print through the active terminal canvas.
        }));
        Self { previous }
    }
}
impl Drop for PanicGuard {
    fn drop(&mut self) {
        if !thread::panicking() {
            let prior = self.previous.clone();
            panic::set_hook(Box::new(move |info| prior(info)));
        }
    }
}

pub struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    modes: ModeGuard,
    _panic: PanicGuard,
    input_enabled: bool,
    enhanced: bool,
}
impl TerminalSession {
    pub fn open(log: Option<Arc<FileLog>>, probe_enhanced_keyboard: bool) -> io::Result<Self> {
        let modes = ModeGuard::acquire()?;
        let panic_guard = PanicGuard::install(log);
        let enhanced =
            probe_enhanced_keyboard && terminal::supports_keyboard_enhancement().unwrap_or(false);
        modes.enter(enhanced)?;
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        Ok(Self {
            terminal,
            modes,
            _panic: panic_guard,
            input_enabled: true,
            enhanced,
        })
    }
    pub fn poll(&mut self, timeout: Duration) -> io::Result<Option<InputEvent>> {
        if !self.input_enabled {
            return Ok(None);
        }
        if !event::poll(timeout)? {
            return Ok(None);
        }
        Ok(normalize(event::read()?))
    }
    pub fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> io::Result<()> {
        if !self.input_enabled {
            return Ok(());
        }
        self.terminal.draw(|frame| {
            if !frame.area().is_empty() {
                render(frame);
            }
        })?;
        Ok(())
    }
    pub fn suspend(&mut self) -> io::Result<()> {
        self.input_enabled = false;
        self.modes.leave()
    }
    pub fn resume(&mut self) -> io::Result<()> {
        if self.input_enabled {
            return Ok(());
        }
        self.modes.enter(self.enhanced)?;
        // Recreate only the rendering buffers. Terminal::clear preserves the
        // physical cursor by querying it, which can block in SSH/browser/PTY
        // sessions that do not answer device queries.
        execute!(io::stdout(), terminal::Clear(terminal::ClearType::All))?;
        self.terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
        self.input_enabled = true;
        Ok(())
    }
    pub fn with_suspended<T>(&mut self, action: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
        self.suspend()?;
        let result = action();
        self.resume()?;
        result
    }
    pub fn enhanced_keyboard(&self) -> bool {
        self.enhanced
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.input_enabled = false;
        let _ = self.modes.leave();
    }
}
