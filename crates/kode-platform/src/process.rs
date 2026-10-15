//! Child ownership: bounded streams, cancellation, termination and reaping.
use crate::jobs::CancellationToken;
use kode_core::{DomainError, Result};
use std::{
    cell::Cell,
    collections::VecDeque,
    path::PathBuf,
    process::{ExitStatus, Stdio},
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
};

pub const STDERR_CAPACITY: usize = 64 * 1024;
pub const OUTPUT_CAPACITY: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
}
impl ProcessSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: vec![],
            cwd: None,
            env: vec![],
        }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .envs(self.env.iter().map(|(key, value)| (key, value)));
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        command
    }
}

pub trait ProcessFactory: Send + Sync {
    fn spawn(&self, spec: &ProcessSpec) -> Result<Child>;
}
pub struct SystemProcessFactory;
impl ProcessFactory for SystemProcessFactory {
    fn spawn(&self, spec: &ProcessSpec) -> Result<Child> {
        spec.command()
            .spawn()
            .map_err(|error| process_error(spec, error))
    }
}

fn process_error(spec: &ProcessSpec, error: impl std::fmt::Display) -> DomainError {
    DomainError::Process {
        program: spec.program.display().to_string(),
        message: error.to_string(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct TailBuffer {
    bytes: Arc<Mutex<VecDeque<u8>>>,
    capacity: usize,
}
impl TailBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            bytes: Arc::default(),
            capacity,
        }
    }
    pub fn append(&self, bytes: &[u8]) {
        let mut tail = self.bytes.lock().expect("stderr mutex");
        for &byte in bytes {
            if self.capacity == 0 {
                break;
            }
            if tail.len() == self.capacity {
                tail.pop_front();
            }
            tail.push_back(byte);
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        self.bytes
            .lock()
            .expect("stderr mutex")
            .iter()
            .copied()
            .collect()
    }
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }
}

async fn drain_tail<R: AsyncRead + Unpin>(mut reader: R, tail: TailBuffer) -> std::io::Result<()> {
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            return Ok(());
        }
        tail.append(&buffer[..count]);
    }
}

pub struct ManagedChild {
    spec: ProcessSpec,
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: TailBuffer,
    stderr_task: JoinHandle<std::io::Result<()>>,
    group: ProcessGroup,
}

struct ProcessGroup(Cell<Option<u32>>);
impl ProcessGroup {
    fn kill(&self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(id) = self.0.take() {
            use nix::{
                errno::Errno,
                sys::signal::{Signal, killpg},
                unistd::Pid,
            };
            match killpg(Pid::from_raw(id as i32), Signal::SIGKILL) {
                Ok(()) | Err(Errno::ESRCH) => {}
                Err(error) => {
                    self.0.set(Some(id));
                    return Err(std::io::Error::from_raw_os_error(error as i32));
                }
            }
        }
        Ok(())
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}
impl ManagedChild {
    pub fn spawn(spec: ProcessSpec, factory: &dyn ProcessFactory) -> Result<Self> {
        let mut child = factory.spawn(&spec)?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = TailBuffer::new(STDERR_CAPACITY);
        let error_pipe = child
            .stderr
            .take()
            .ok_or_else(|| process_error(&spec, "stderr pipe missing"))?;
        let tail = stderr.clone();
        let stderr_task = tokio::spawn(async move { drain_tail(error_pipe, tail).await });
        let group = ProcessGroup(Cell::new(child.id()));
        Ok(Self {
            spec,
            child,
            stdin,
            stdout,
            stderr,
            stderr_task,
            group,
        })
    }
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub fn stderr(&self) -> String {
        self.stderr.text()
    }
    pub fn stderr_tail(&self) -> TailBuffer {
        self.stderr.clone()
    }
    /// Supervise an interactive child after its pipes have been transferred.
    /// Cancellation kills its owned process group, then reaps and joins stderr.
    pub async fn wait_or_cancel(mut self, token: CancellationToken) -> Result<(ExitStatus, bool)> {
        self.stdin.take();
        self.stdout.take();
        let (status, cancelled) = tokio::select! {
            biased;
            _=token.cancelled()=>{
                self.group.kill().map_err(|error|process_error(&self.spec,error))?;
                if self.child.try_wait().map_err(|error|process_error(&self.spec,error))?.is_none() { self.child.start_kill().map_err(|error|process_error(&self.spec,error))?; }
                (self.child.wait().await.map_err(|error|process_error(&self.spec,error))?,true)
            },
            status=self.child.wait()=>(status.map_err(|error|process_error(&self.spec,error))?,false),
        };
        self.group
            .kill()
            .map_err(|error| process_error(&self.spec, error))?;
        self.stderr_task
            .await
            .map_err(|error| process_error(&self.spec, error))?
            .map_err(|error| process_error(&self.spec, error))?;
        Ok((status, cancelled))
    }
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.stdin.take()
    }
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.stdout.take()
    }
    pub async fn terminate_and_reap(mut self) -> Result<ExitStatus> {
        self.stdin.take();
        self.stdout.take();
        self.group
            .kill()
            .map_err(|error| process_error(&self.spec, error))?;
        if self
            .child
            .try_wait()
            .map_err(|error| process_error(&self.spec, error))?
            .is_none()
        {
            self.child
                .start_kill()
                .map_err(|error| process_error(&self.spec, error))?;
        }
        let status = self
            .child
            .wait()
            .await
            .map_err(|error| process_error(&self.spec, error))?;
        self.group
            .kill()
            .map_err(|error| process_error(&self.spec, error))?;
        self.stderr_task
            .await
            .map_err(|error| process_error(&self.spec, error))?
            .map_err(|error| process_error(&self.spec, error))?;
        Ok(status)
    }
    pub async fn wait(mut self) -> Result<ExitStatus> {
        // Close owned input so a child waiting for EOF can finish. If the caller
        // has not taken stdout, drain it concurrently to avoid a full-pipe hang.
        self.stdin.take();
        let stdout = self
            .stdout
            .take()
            .map(|reader| tokio::spawn(drain_tail(reader, TailBuffer::new(0))));
        let status = self
            .child
            .wait()
            .await
            .map_err(|error| process_error(&self.spec, error))?;
        self.group
            .kill()
            .map_err(|error| process_error(&self.spec, error))?;
        if let Some(task) = stdout {
            task.await
                .map_err(|error| process_error(&self.spec, error))?
                .map_err(|error| process_error(&self.spec, error))?;
        }
        self.stderr_task
            .await
            .map_err(|error| process_error(&self.spec, error))?
            .map_err(|error| process_error(&self.spec, error))?;
        Ok(status)
    }
}

#[derive(Debug)]
pub struct ProcessOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
    pub truncated: bool,
    pub cancelled: bool,
}

pub async fn run_capture(
    spec: ProcessSpec,
    input: Vec<u8>,
    token: CancellationToken,
    factory: &dyn ProcessFactory,
) -> Result<ProcessOutput> {
    let mut managed = ManagedChild::spawn(spec.clone(), factory)?;
    let stdin = managed.take_stdin();
    let input_task = tokio::spawn(async move {
        if let Some(mut writer) = stdin {
            writer.write_all(&input).await?;
            writer.shutdown().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let mut reader = managed
        .take_stdout()
        .ok_or_else(|| process_error(&spec, "stdout pipe missing"))?;
    let output_task = tokio::spawn(async move {
        let mut output = Vec::new();
        let mut truncated = false;
        let mut buffer = [0; 8192];
        loop {
            let count = reader.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            let remaining = OUTPUT_CAPACITY.saturating_sub(output.len());
            output.extend_from_slice(&buffer[..count.min(remaining)]);
            truncated |= count > remaining;
        }
        Ok::<_, std::io::Error>((output, truncated))
    });
    let (status, cancelled) = tokio::select! {
        result = managed.child.wait() => (result.map_err(|error| process_error(&spec, error))?, false),
        _ = token.cancelled() => {
            managed.group.kill().map_err(|error| process_error(&spec, error))?;
            managed.child.start_kill().map_err(|error| process_error(&spec, error))?;
            (managed.child.wait().await.map_err(|error| process_error(&spec, error))?, true)
        }
    };
    managed
        .group
        .kill()
        .map_err(|error| process_error(&spec, error))?;
    let input_result = input_task
        .await
        .map_err(|error| process_error(&spec, error))?;
    if let Err(error) = input_result {
        // Early child exit and cancellation legitimately close the input pipe.
        if !cancelled && error.kind() != std::io::ErrorKind::BrokenPipe {
            return Err(process_error(&spec, error));
        }
    }
    let (stdout, truncated) = output_task
        .await
        .map_err(|error| process_error(&spec, error))?
        .map_err(|error| process_error(&spec, error))?;
    managed
        .stderr_task
        .await
        .map_err(|error| process_error(&spec, error))?
        .map_err(|error| process_error(&spec, error))?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr: managed.stderr.text(),
        truncated,
        cancelled,
    })
}
