//! Graceful terminal/SSH/ttyd shutdown signals. SIGKILL remains uncatchable.
pub struct Shutdown {
    #[cfg(unix)]
    hangup: tokio::signal::unix::Signal,
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
}
impl Shutdown {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(unix)]
            hangup: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())?,
            #[cfg(unix)]
            terminate: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?,
        })
    }
    pub async fn recv(&mut self) {
        #[cfg(unix)]
        tokio::select! {
            _ = self.hangup.recv() => {},
            _ = self.terminate.recv() => {},
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    }
}
