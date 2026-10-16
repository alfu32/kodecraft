use super::{runtime::AppRuntime, workspace::prepare_workspace};
use crate::{terminal::TerminalSession, theme::Theme};
use kode_core::{DomainError, Result, WorkspaceGeneration};
use kode_platform::{
    jobs::{CancellationToken, MonotonicClock},
    logging::FileLog,
    paths::{ResourcePaths, compiled_resources},
};
use kode_workspace::mime::MimeDetector;
use std::sync::Arc;
use std::{path::PathBuf, time::Duration};

fn terminal_error(source: std::io::Error) -> DomainError {
    DomainError::Io {
        operation: "terminal session",
        path: PathBuf::from("<terminal>"),
        source,
    }
}
pub async fn run(root: PathBuf, initial_file: Option<PathBuf>) -> Result<()> {
    run_with_resources(root, initial_file, None).await
}
pub async fn run_with_resources(
    root: PathBuf,
    initial_file: Option<PathBuf>,
    resources: Option<PathBuf>,
) -> Result<()> {
    let (prepared, theme, detector, log, intelligence, lsp, database) =
        tokio::task::spawn_blocking(move || {
            let paths = ResourcePaths::from_environment(resources)?;
            let defaults = compiled_resources();
            let css = paths.resolve("styles/app.css", &defaults)?;
            let css = std::str::from_utf8(&css.bytes).map_err(|error| DomainError::Parse {
                format: "theme UTF-8",
                message: error.to_string(),
            })?;
            let mut theme = Theme::compiled();
            theme.merge(&Theme::parse(css));
            let detector =
                MimeDetector::from_table(&paths.resolve("mime-types.json", &defaults)?.bytes)?;
            let definitions = std::env::var_os("KODE_CODEINTEL_PATH").map(PathBuf::from);
            let intelligence = Arc::new(kode_intelligence::facade::IntelligenceResources::load(
                &paths,
                definitions.as_deref(),
            )?);
            let log = Arc::new(FileLog::open(
                &paths.config_dir.join("kode.jsonl"),
                4 * 1024 * 1024,
            )?);
            let prepared = prepare_workspace(
                &root,
                WorkspaceGeneration(1),
                initial_file,
                &CancellationToken::new(),
            )?;
            let lsp = super::runtime::lsp::LspConfiguration::from_environment(&paths);
            let database =
                super::runtime::database::DatabaseConfiguration::from_environment(&paths);
            Ok::<_, DomainError>((prepared, theme, detector, log, intelligence, lsp, database))
        })
        .await
        .map_err(|error| {
            DomainError::Unsupported(format!("workspace startup failed: {error}"))
        })??;
    let mut runtime = AppRuntime::new(prepared)?;
    runtime.set_detector(detector)?;
    runtime.start_intelligence(intelligence).await?;
    runtime.configure_lsp(lsp)?;
    if let Err(error) = database.and_then(|config| runtime.configure_database(config)) {
        runtime.database_configuration_failed(error);
    }
    let terminal = TerminalSession::open(Some(log.clone()), true).map_err(terminal_error);
    let mut terminal = match terminal {
        Ok(terminal) => terminal,
        Err(error) => {
            runtime.terminal_failed().await?;
            return Err(error);
        }
    };
    let result = event_loop(&mut runtime, &mut terminal, &theme).await;
    let cleanup = if result.is_err() {
        runtime.terminal_failed().await
    } else {
        Ok(())
    };
    drop(terminal);
    let message = match (&result, &cleanup) {
        (Err(error), Err(cleanup)) => format!("{error}; {cleanup}"),
        (Err(error), _) => error.to_string(),
        _ => "Workspace closed".into(),
    };
    let level = if result.is_err() { "error" } else { "info" };
    tokio::task::spawn_blocking(move || {
        log.record(
            &MonotonicClock::default(),
            level,
            "application",
            &message,
            serde_json::json!({}),
        )
    })
    .await
    .map_err(|error| DomainError::Unsupported(format!("log cleanup failed: {error}")))??;
    cleanup?;
    result
}
async fn event_loop(
    runtime: &mut AppRuntime,
    terminal: &mut TerminalSession,
    theme: &Theme,
) -> Result<()> {
    let mut repaint = runtime.notifications.subscribe();
    loop {
        if let Some(event) = terminal.poll(Duration::ZERO).map_err(terminal_error)?
            && let Err(error) = runtime.state.dispatch(&event)
        {
            runtime.state.notice = error.to_string();
        }
        if runtime.state.switch_requested.is_some() {
            runtime.state.closing = true;
            runtime.state.pending_open = None;
        }
        if let Err(error) = runtime.pump() {
            runtime.state.notice = error.to_string();
        }
        let mut paint_error = None;
        terminal
            .draw(|frame| {
                let area = frame.area();
                match runtime.state.render(area, frame.buffer_mut(), theme) {
                    Ok(Some(cursor)) if runtime.state.window_focused => {
                        frame.set_cursor_position(cursor)
                    }
                    Err(error) => paint_error = Some(error),
                    _ => {}
                }
            })
            .map_err(terminal_error)?;
        if let Some(error) = paint_error {
            runtime.state.notice = error.to_string();
        }
        if runtime.ready_to_leave() {
            if runtime.state.quit_requested {
                runtime.quiesce().await?;
                return Ok(());
            }
            if let Some(root) = runtime.state.switch_requested.take() {
                if let Err(error) = runtime.switch_workspace(root).await {
                    runtime.state.notice = error.to_string();
                }
                // Keystrokes queued while the old workspace was closing belong
                // to that workspace; never type them into a freshly restored file.
                for _ in 0..64 {
                    if terminal
                        .poll(Duration::ZERO)
                        .map_err(terminal_error)?
                        .is_none()
                    {
                        break;
                    }
                }
            }
        }
        // Worker results wake painting directly. The timer services input,
        // hover/drag and debounce deadlines even when no terminal event arrives.
        tokio::select! {
            _ = repaint.changed() => {},
            _ = tokio::time::sleep(Duration::from_millis(16)) => {},
        }
    }
}
