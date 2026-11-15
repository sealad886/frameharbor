//! Bounded line/binary IPC and owned-child shutdown shared by native helpers.
//! One queued event/response, fallible bounded-stack threads, deadline/cancel
//! checks and actual wait receipts. This is not an OS security sandbox.
use std::{
    fmt,
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
#[derive(Debug)]
pub(crate) enum SupervisionError {
    Io(io::Error),
    Protocol,
    ResourceLimit,
    Cancelled,
    TimedOut,
    NativeFailure,
}
impl fmt::Display for SupervisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for SupervisionError {}
impl From<io::Error> for SupervisionError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
type Result<T> = std::result::Result<T, SupervisionError>;
pub(crate) enum Event {
    Line(Vec<u8>),
    End,
    Invalid,
    Io,
}
/// Cleanup evidence for the child handle owned by this adapter. This does not
/// certify descendant processes or constitute an operating-system sandbox.
#[derive(Debug, Clone)]
pub struct ChildCleanup {
    pub pid: u32,
    pub reaped: bool,
    pub status: Option<ExitStatus>,
    pub kill_error: Option<io::ErrorKind>,
    pub wait_error: Option<io::ErrorKind>,
}
pub(crate) struct Worker<'a> {
    pub(crate) child: Child,
    input: Option<SyncSender<Vec<u8>>>,
    reader: Option<JoinHandle<()>>,
    writer: Option<JoinHandle<()>>,
    stopping: Arc<AtomicBool>,
    cleanup: Option<Box<dyn FnOnce(ChildCleanup) + 'a>>,
}
impl Drop for Worker<'_> {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Relaxed);
        let pid = self.child.id();
        let kill_error = self.child.kill().err().map(|e| e.kind());
        let (reaped, status, wait_error) = match self.child.wait() {
            Ok(status) => (true, Some(status), None),
            Err(e) => (false, None, Some(e.kind())),
        };
        self.input.take();
        if let Some(join) = self.reader.take() {
            let _ = join.join();
        }
        if let Some(join) = self.writer.take() {
            let _ = join.join();
        }
        if let Some(observer) = self.cleanup.take() {
            observer(ChildCleanup {
                pid,
                reaped,
                status,
                kill_error,
                wait_error,
            });
        }
    }
}
pub(crate) fn spawn<'a>(
    command: &mut Command,
    output_limit: usize,
    cleanup: impl FnOnce(ChildCleanup) + 'a,
) -> Result<(Worker<'a>, Receiver<Event>)> {
    if output_limit == 0 || output_limit > 64 * 1024 * 1024 {
        return Err(SupervisionError::ResourceLimit);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdout = child.stdout.take().ok_or(SupervisionError::Protocol)?;
    let mut stdin = child.stdin.take().ok_or(SupervisionError::Protocol)?;
    let (events, receive) = mpsc::sync_channel(1);
    let (input, requests) = mpsc::sync_channel::<Vec<u8>>(1);
    let stopping = Arc::new(AtomicBool::new(false));
    let reader_stop = stopping.clone();
    // Establish cleanup ownership before fallible thread creation.
    let mut worker = Worker {
        child,
        input: Some(input),
        reader: None,
        writer: None,
        stopping,
        cleanup: Some(Box::new(cleanup)),
    };
    let reader = thread::Builder::new()
        .name("frameharbor-native-read".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            let mut source = BufReader::with_capacity(4096, stdout);
            let mut total = 0usize;
            loop {
                let mut line = Vec::new();
                let read = Read::by_ref(&mut source)
                    .take(4097)
                    .read_until(b'\n', &mut line);
                match read {
                    Ok(0) => {
                        emit_event(&events, Event::End, &reader_stop);
                        break;
                    }
                    Ok(n) => {
                        total = total.saturating_add(n);
                        if n > 4096 || total > output_limit || !line.ends_with(b"\n") {
                            emit_event(&events, Event::Invalid, &reader_stop);
                            break;
                        }
                        if !emit_event(&events, Event::Line(line), &reader_stop) {
                            break;
                        }
                    }
                    Err(_) => {
                        emit_event(&events, Event::Io, &reader_stop);
                        break;
                    }
                }
            }
        })?;
    worker.reader = Some(reader);
    let writer = thread::Builder::new()
        .name("frameharbor-native-write".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            while let Ok(bytes) = requests.recv() {
                if stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        })?;
    worker.writer = Some(writer);
    Ok((worker, receive))
}
fn emit_event(sender: &SyncSender<Event>, mut event: Event, stop: &AtomicBool) -> bool {
    while !stop.load(Ordering::Relaxed) {
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Full(value)) => {
                event = value;
                thread::sleep(Duration::from_millis(2));
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
    false
}
pub(crate) fn check_control(
    cancel: &AtomicBool,
    started: Instant,
    allowance: Duration,
) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(SupervisionError::Cancelled)
    } else if started.elapsed() >= allowance {
        Err(SupervisionError::TimedOut)
    } else {
        Ok(())
    }
}
pub(crate) fn send(
    worker: &Worker<'_>,
    mut bytes: Vec<u8>,
    cancel: &AtomicBool,
    started: Instant,
    allowance: Duration,
) -> Result<()> {
    loop {
        check_control(cancel, started, allowance)?;
        match worker
            .input
            .as_ref()
            .ok_or(SupervisionError::Protocol)?
            .try_send(bytes)
        {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(value)) => {
                bytes = value;
                thread::sleep(Duration::from_millis(2));
            }
            Err(TrySendError::Disconnected(_)) => return Err(SupervisionError::NativeFailure),
        }
    }
}
