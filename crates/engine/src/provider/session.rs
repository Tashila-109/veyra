use super::{
    ProviderError, ProviderEvent, ProviderKind, SessionControl, SessionEvent, SessionHandle,
    SessionSpec, StopReason, TurnEvent,
    claude::{self, Codec},
    env::ChildEnv,
};
use crate::{
    actor::EngineMsg,
    domain::*,
    platform::{self, Signal},
    record,
};
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    process::Child,
    sync::{mpsc, oneshot},
};

pub(crate) struct ProcessGroup {
    child: Option<Child>,
    pid: u32,
    reaped: Option<oneshot::Sender<()>>,
    groups: Option<super::env::Groups>,
    #[cfg(test)]
    check: fn(&mut Child, u32) -> std::io::Result<bool>,
}
fn process_alive(child: &mut Child, pid: u32) -> std::io::Result<bool> {
    child.try_wait()?;
    platform::group_exists(pid)
}
async fn wait_for_group(
    child: &mut Child,
    pid: u32,
    check: fn(&mut Child, u32) -> std::io::Result<bool>,
) -> std::io::Result<bool> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    loop {
        if !check(child, pid)? {
            return Ok(false);
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(true);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
async fn end_process(
    mut child: Child,
    pid: u32,
    check: fn(&mut Child, u32) -> std::io::Result<bool>,
) -> std::io::Result<std::process::ExitStatus> {
    let _ = platform::signal_group(pid, Signal::Terminate);
    let mut failure = None;
    match wait_for_group(&mut child, pid, check).await {
        Ok(false) => {}
        result => {
            failure = result.err();
            let _ = platform::signal_group(pid, Signal::Kill);
        }
    }
    let status = child.wait().await;
    match wait_for_group(&mut child, pid, check).await {
        Ok(false) => {}
        Ok(true) => tracing::warn!(
            pgid = pid,
            "Provider process group still exists after cleanup deadline"
        ),
        Err(error) => {
            failure.get_or_insert(error);
        }
    }
    failure.map_or(status, Err)
}
impl ProcessGroup {
    pub(crate) fn pid(&self) -> u32 {
        self.pid
    }
    pub(crate) fn tracked(child: Child, env: &ChildEnv) -> Self {
        let pid = child.id().expect("new child");
        env.groups.lock().unwrap().insert(pid);
        Self {
            pid,
            child: Some(child),
            reaped: None,
            groups: Some(env.groups.clone()),
            #[cfg(test)]
            check: process_alive,
        }
    }
    fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }
    fn stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.as_mut()?.stderr.take()
    }
    fn stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.child.as_mut()?.stdin.take()
    }
    #[cfg(test)]
    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.as_mut().unwrap().wait().await
    }
    fn watch(&mut self, cleanup: &mpsc::UnboundedSender<oneshot::Receiver<()>>) {
        let (sender, receiver) = oneshot::channel();
        self.reaped = Some(sender);
        let _ = cleanup.send(receiver);
    }
    fn end(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<std::io::Result<std::process::ExitStatus>>> {
        let child = self.child.take()?;
        let pid = self.pid;
        let reaped = self.reaped.take();
        let groups = self.groups.take();
        #[cfg(test)]
        let check = self.check;
        #[cfg(not(test))]
        let check = process_alive;
        Some(tokio::spawn(async move {
            let result = end_process(child, pid, check).await;
            if let Some(groups) = groups {
                groups.lock().unwrap().remove(&pid);
            }
            drop(reaped);
            result
        }))
    }
    pub(crate) async fn stop(mut self) -> std::io::Result<std::process::ExitStatus> {
        match self.end() {
            Some(task) => task.await.map_err(std::io::Error::other)?,
            None => Err(std::io::Error::other("process already stopped")),
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if self.child.is_some() {
            let _ = platform::signal_group(self.pid, Signal::Kill);
        }
        self.end();
    }
}
struct WriterLine {
    line: serde_json::Value,
    interrupt: Option<TurnId>,
}
async fn write_interrupt(
    stdin: &mut (impl AsyncWrite + Unpin),
    bytes: &[u8],
    turn: TurnId,
    active: &Mutex<Option<TurnId>>,
) -> std::io::Result<()> {
    let written = std::future::poll_fn(|cx| {
        let active = active.lock().unwrap();
        if *active != Some(turn) {
            return std::task::Poll::Ready(Ok(None));
        }
        std::pin::Pin::new(&mut *stdin)
            .poll_write(cx, bytes)
            .map(|result| result.map(Some))
    })
    .await?;
    if let Some(written) = written {
        if written == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        stdin.write_all(&bytes[written..]).await?;
    }
    Ok(())
}
struct Process {
    child: ProcessGroup,
    native: NativeSessionId,
    #[cfg(debug_assertions)]
    recorder: Option<record::Recorder>,
}
impl Process {
    fn send(
        &mut self,
        writer: &mpsc::UnboundedSender<WriterLine>,
        line: serde_json::Value,
        turn: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            recorder.line(true, line.clone())?;
        }
        let interrupt = if line["request"]["subtype"] == "interrupt" {
            turn
        } else {
            None
        };
        writer
            .send(WriterLine { line, interrupt })
            .map_err(|_| ProviderError::Protocol("Claude stdin writer closed".into()))
    }
    fn record_inbound(&mut self, line: &serde_json::Value) -> Result<(), ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            recorder.line(false, line.clone())?;
        }
        #[cfg(not(debug_assertions))]
        let _ = line;
        Ok(())
    }
}
fn launch(
    spec: &SessionSpec,
    env: &ChildEnv,
    fresh: Option<NativeSessionId>,
) -> Result<Process, ProviderError> {
    let binary = env.resolve(ProviderKind::Claude)?;
    #[cfg(debug_assertions)]
    let (n, epoch) = env.next_lane(spec.thread);
    let mut args = claude::args(Some(spec));
    let native = fresh.unwrap_or_else(|| {
        spec.native
            .clone()
            .unwrap_or_else(|| NativeSessionId(spec.thread.to_string()))
    });
    if spec.native.is_none()
        && let Some(index) = args.iter().position(|arg| arg == "--session-id")
    {
        args[index + 1] = native.0.clone();
    }
    #[cfg(debug_assertions)]
    let fixture = env
        .fixtures
        .as_ref()
        .map(|root| root.join(format!("t{n}-s{epoch}.jsonl")));
    #[cfg(not(debug_assertions))]
    let fixture: Option<std::path::PathBuf> = None;
    #[cfg(debug_assertions)]
    let recorder = env
        .record
        .as_ref()
        .map(|root| {
            record::Recorder::create(
                &root.join(format!("t{n}-s{epoch}.jsonl")),
                "claude",
                "unknown until system/init",
                &spec.cwd,
                env.home.as_deref(),
                None,
            )
        })
        .transpose()?;
    let child = env.spawn(&binary, &args, &spec.cwd, fixture.as_deref())?;
    Ok(Process {
        child,
        native,
        #[cfg(debug_assertions)]
        recorder,
    })
}
pub fn spawn(
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
) -> Result<SessionHandle, ProviderError> {
    env.resolve(ProviderKind::Claude)?;
    Ok(start_task(None, spec, env, out, epoch))
}
#[cfg(test)]
fn start_session(
    process: Process,
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
) -> SessionHandle {
    start_task(Some(process), spec, env, out, epoch)
}
fn start_task(
    process: Option<Process>,
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
) -> SessionHandle {
    let (control, receiver) = mpsc::unbounded_channel();
    let (cleanup, reaped) = mpsc::unbounded_channel();
    let thread = spec.thread;
    let supervisor_out = out.clone();
    let task = tokio::spawn(async move {
        let mut spec = spec;
        let mut receiver = SessionControls::new(receiver, env.closing.clone());
        #[cfg(test)]
        {
            receiver.close_before_user_write = env.close_before_user_write.clone();
        }
        let mut process = match process {
            Some(process) => process,
            None => {
                match receiver
                    .drain_and_check_launch(&out)
                    .and_then(|()| launch(&spec, &env, None))
                {
                    Ok(process) => process,
                    Err(error) => {
                        return emit(&out, thread, epoch, launch_failed(error)).await;
                    }
                }
            }
        };
        process.child.watch(&cleanup);
        loop {
            match run_one(
                process,
                &mut receiver,
                &out,
                thread,
                epoch,
                spec.native.is_some(),
                &spec.cwd,
            )
            .await
            {
                Completion::MissingSession => {
                    spec.native = None;
                    let native = NativeSessionId(ThreadId::new().to_string());
                    match receiver
                        .drain_and_check_launch(&out)
                        .and_then(|()| launch(&spec, &env, Some(native)))
                    {
                        Ok(mut next) => {
                            next.child.watch(&cleanup);
                            let notice = ProviderEvent::Session(SessionEvent::Notice {
                                text: "The earlier conversation could not be resumed; this is a new session".into(),
                            });
                            if !emit(&out, thread, epoch, notice).await {
                                return false;
                            }
                            process = next;
                        }
                        Err(error) => {
                            return emit(&out, thread, epoch, launch_failed(error)).await;
                        }
                    }
                }
                Completion::Exited(crash) => {
                    return emit(
                        &out,
                        thread,
                        epoch,
                        ProviderEvent::Session(SessionEvent::Exited { crash }),
                    )
                    .await;
                }
                Completion::ConsumerGone => return false,
            }
        }
    });
    tokio::spawn(supervise(task, reaped, supervisor_out, thread, epoch));
    SessionHandle { control }
}
fn launch_failed(error: ProviderError) -> ProviderEvent {
    ProviderEvent::Session(SessionEvent::Exited {
        crash: if matches!(error, ProviderError::Cancelled) {
            None
        } else {
            Some(record::redact(&error.to_string(), None))
        },
    })
}

async fn supervise(
    task: tokio::task::JoinHandle<bool>,
    mut reaped: mpsc::UnboundedReceiver<oneshot::Receiver<()>>,
    out: mpsc::Sender<EngineMsg>,
    thread: ThreadId,
    epoch: u32,
) {
    let result = task.await;
    while let Some(done) = reaped.recv().await {
        let _ = done.await;
    }
    if let Err(error) = result {
        let _ = emit(
            &out,
            thread,
            epoch,
            ProviderEvent::Session(SessionEvent::Exited {
                crash: Some(record::redact(
                    &format!("Claude session task failed: {error}"),
                    None,
                )),
            }),
        )
        .await;
    }
}
async fn emit(
    out: &mpsc::Sender<EngineMsg>,
    thread: ThreadId,
    epoch: u32,
    event: ProviderEvent,
) -> bool {
    out.send(EngineMsg::Provider {
        thread,
        epoch,
        event,
    })
    .await
    .is_ok()
}
const LINE_CAP: usize = 4 * 1024 * 1024;
enum Line {
    Text(String),
    Oversize(usize),
}
struct ByteLines<R> {
    reader: BufReader<R>,
    bytes: Vec<u8>,
    length: usize,
}
impl<R: AsyncRead + Unpin> ByteLines<R> {
    fn new(reader: R) -> Self {
        Self {
            reader: BufReader::new(reader),
            bytes: Vec::new(),
            length: 0,
        }
    }
    async fn next(&mut self) -> std::io::Result<Option<Line>> {
        loop {
            let available = self.reader.fill_buf().await?;
            let end = available.iter().position(|byte| *byte == b'\n');
            let count = end.unwrap_or(available.len());
            let eof = available.is_empty();
            self.length = self.length.saturating_add(count);
            if self.length <= LINE_CAP {
                self.bytes.extend_from_slice(&available[..count]);
            } else {
                self.bytes.clear();
            }
            self.reader.consume(count + usize::from(end.is_some()));
            if end.is_some() || eof {
                let length = std::mem::take(&mut self.length);
                let bytes = std::mem::take(&mut self.bytes);
                return Ok(if eof && length == 0 {
                    None
                } else if length > LINE_CAP {
                    Some(Line::Oversize(length))
                } else {
                    Some(Line::Text(String::from_utf8_lossy(&bytes).into_owned()))
                });
            }
        }
    }
}
struct SessionControls {
    receiver: mpsc::UnboundedReceiver<SessionControl>,
    pending: VecDeque<SessionControl>,
    closing: Arc<crate::LaunchGate>,
    #[cfg(test)]
    close_before_user_write: Arc<AtomicBool>,
}
impl SessionControls {
    fn new(
        receiver: mpsc::UnboundedReceiver<SessionControl>,
        closing: Arc<crate::LaunchGate>,
    ) -> Self {
        Self {
            receiver,
            pending: VecDeque::new(),
            closing,
            #[cfg(test)]
            close_before_user_write: Default::default(),
        }
    }
    fn drain_and_close(&mut self) {
        if self.closing.is_closing() {
            self.receiver.close();
        }
        while let Ok(message) = self.receiver.try_recv() {
            if matches!(message, SessionControl::Shutdown) {
                self.receiver.close();
            }
            self.pending.push_back(message);
        }
    }
    fn drain_and_check_launch(
        &mut self,
        out: &mpsc::Sender<EngineMsg>,
    ) -> Result<(), ProviderError> {
        self.drain_and_close();
        if self.receiver.is_closed() || out.is_closed() {
            Err(ProviderError::Cancelled)
        } else {
            Ok(())
        }
    }
    async fn recv(&mut self) -> Option<SessionControl> {
        let message = match self.pending.pop_front() {
            Some(message) => Some(message),
            None => self.receiver.recv().await,
        };
        if matches!(message, Some(SessionControl::StartTurn { .. })) {
            self.drain_and_close();
            if self.receiver.is_closed() {
                return Some(SessionControl::Shutdown);
            }
        }
        if matches!(message, Some(SessionControl::Shutdown) | None) {
            self.receiver.close();
        }
        message
    }
}

enum Completion {
    MissingSession,
    Exited(Option<String>),
    ConsumerGone,
}

async fn run_one(
    mut process: Process,
    control: &mut SessionControls,
    out: &mpsc::Sender<EngineMsg>,
    thread: ThreadId,
    epoch: u32,
    resume: bool,
    cwd: &std::path::Path,
) -> Completion {
    let pid = process.child.pid();
    let mut lines = ByteLines::new(process.child.stdout().expect("piped stdout"));
    let mut errors = ByteLines::new(process.child.stderr().expect("piped stderr"));
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stderr_tail = tail.clone();
    let mut stderr = tokio::spawn(async move {
        while let Ok(Some(line)) = errors.next().await {
            let line = match line {
                Line::Text(line) => line,
                Line::Oversize(length) => format!("Claude stderr line skipped ({length} bytes)"),
            };
            let mut tail = stderr_tail.lock().unwrap();
            tail.push_back(line);
            while tail.len() > 8 {
                tail.pop_front();
            }
        }
    });
    let mut stdin = process.child.stdin().expect("piped stdin");
    let (writer, mut input) = mpsc::unbounded_channel::<WriterLine>();
    let active_turn = Arc::new(Mutex::new(None));
    let writer_turn = active_turn.clone();
    let writer_closing = control.closing.clone();
    #[cfg(test)]
    let close_before_user_write = control.close_before_user_write.clone();
    let mut write_task = tokio::spawn(async move {
        while let Some(WriterLine { line, interrupt }) = input.recv().await {
            let mut bytes = serde_json::to_vec(&line)?;
            bytes.push(b'\n');
            #[cfg(test)]
            if line["type"] == "user" && close_before_user_write.swap(false, Ordering::AcqRel) {
                writer_closing.close();
            }
            if line["type"] == "user" && writer_closing.is_closing() {
                continue;
            }
            if let Some(turn) = interrupt {
                write_interrupt(&mut stdin, &bytes, turn, &writer_turn).await?;
            } else {
                stdin.write_all(&bytes).await?;
            }
            stdin.flush().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let mut codec = Codec::new(process.native.clone(), pid, cwd);
    let mut ready = false;
    let mut rejected = None;
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut process.recorder {
        recorder.bind(process.native.0.clone(), "<id:session>".into());
    }
    let mut failure = process
        .send(&writer, claude::initialize(), None)
        .err()
        .map(|error| error.to_string());
    let mut consumer_gone = false;
    let mut writer_done = false;
    let mut stdout_closed = false;
    while failure.is_none() {
        tokio::select! {
            biased;
            _ = out.closed() => { consumer_gone = true; break; }
            message = control.recv() => match message {
                Some(SessionControl::StartTurn { turn, text }) => {
                    let Some(line) = codec.start(turn, text) else {
                        if !emit(out, thread, epoch, ProviderEvent::Session(
                            SessionEvent::StartTurnRefused { turn }
                        )).await { consumer_gone = true; break; }
                        continue;
                    };
                    #[cfg(debug_assertions)]
                    if let Some(recorder) = &mut process.recorder {
                        recorder.bind(turn.to_string(), format!("<id:turn:{turn}>"));
                    }
                    *active_turn.lock().unwrap() = codec.active.map(claude::ActiveTurn::target);
                    if let Err(error) = process.send(&writer, line, None) {
                        failure = Some(error.to_string());
                        break;
                    }
                }
                Some(SessionControl::Shutdown) | None => {
                    let turn = codec.active.map(claude::ActiveTurn::target);
                    let lines = codec.interrupt_active(StopReason::AppClosing);
                    for line in lines {
                        if let Err(error) = process.send(&writer, line, turn) {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                    drop(writer);
                    if tokio::time::timeout(Duration::from_secs(1), &mut write_task).await.is_ok() {
                        writer_done = true;
                    }
                    break;
                }
                Some(SessionControl::Interrupt { turn, reason }) => {
                    for line in codec.interrupt_lines(turn, reason) {
                        if let Err(error) = process.send(&writer, line, Some(turn)) {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
                Some(SessionControl::InterruptActive(reason)) => {
                    let turn = codec.active.map(claude::ActiveTurn::target);
                    for line in codec.interrupt_active(reason) {
                        if let Err(error) = process.send(&writer, line, turn) {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
                Some(SessionControl::Answer { native, reply }) => {
                    if let Some(line) = codec.answer(native, reply)
                        && let Err(error) = process.send(&writer, line, None)
                    {
                        failure = Some(error.to_string());
                        break;
                    }
                }
            },
            result = &mut write_task => {
                writer_done = true;
                failure = Some(match result {
                    Ok(Err(error)) => format!("Claude stdin writer failed: {error}"),
                    Err(error) => format!("Claude stdin writer task failed: {error}"),
                    Ok(Ok(())) => "Claude stdin writer closed".into(),
                });
                break;
            }
            _ = async {
                match codec.interrupt_deadline() {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(signal) = codec.escalate_interrupt() {
                    let kill = matches!(signal, Signal::Kill);
                    let _ = platform::signal_group(pid, signal);
                    if kill { break; }
                }
            }
            line = lines.next() => match line {
                Ok(Some(Line::Oversize(length))) => {
                    tracing::info!(length, "skipped oversized Claude line");
                    if !emit(out, thread, epoch, ProviderEvent::Session(SessionEvent::Notice {
                        text: "Claude sent a line larger than 4 MiB; Veyra skipped it.".into()
                    })).await { consumer_gone = true; break; }
                }
                Ok(Some(Line::Text(line))) => {
                    let wire = match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(wire) => wire,
                        Err(error) => {
                            tracing::info!(length = line.len(), "skipped non-JSON Claude line");
                            rejected = Some(ProviderError::Json(error).to_string());
                            continue;
                        }
                    };
                    rejected = None;
                    if let Err(error) = process.record_inbound(&wire) {
                        failure = Some(error.to_string());
                        break;
                    }
                    match codec.value(wire) {
                        Ok(decoded) => {
                            *active_turn.lock().unwrap() = codec.active.map(claude::ActiveTurn::target);
                            for reply in decoded.replies {
                                if let Err(error) = process.send(&writer, reply, None) {
                                    failure = Some(error.to_string());
                                    break;
                                }
                            }
                            if failure.is_some() { break; }
                            if let Some(provider_turn) = decoded.provider_turn
                                && !emit(out, thread, epoch, ProviderEvent::Session(
                                    SessionEvent::ProviderTurnStarted { provider_turn }
                                )).await
                            { consumer_gone = true; break; }
                            for event in decoded.events {
                                if matches!(event, ProviderEvent::Session(SessionEvent::Ready { .. })) {
                                    ready = true;
                                }
                                if !emit(out, thread, epoch, event).await { consumer_gone = true; break; }
                            }
                            if consumer_gone { break; }
                        }
                        Err(error) => { failure = Some(error.to_string()); break; }
                    }
                }
                Ok(None) => { stdout_closed = true; break; },
                Err(error) => { failure = Some(error.to_string()); break; }
            }
        }
    }
    if !writer_done {
        write_task.abort();
        let _ = write_task.await;
    }
    let observed = process
        .child
        .child
        .as_mut()
        .and_then(|child| child.try_wait().ok().flatten());
    let wait = async {
        match process.child.child.as_mut() {
            Some(child) => child.wait().await,
            None => std::future::pending().await,
        }
    };
    let observed = drain_stderr(&mut stderr, wait, observed, stdout_closed).await;
    let lines = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>();
    let failure = failure
        .or(rejected)
        .map(|text| with_stderr(&text, &lines, None));
    let missing = resume
        && lines
            .iter()
            .any(|line| line.contains("No conversation found with session ID"));
    let crash = if consumer_gone || missing {
        None
    } else if let Some(active) = codec.active {
        let outside = observed
            .as_ref()
            .and_then(|status| match platform::exit_signal(status) {
                Some(signal @ ("SIGHUP" | "SIGINT" | "SIGTERM")) => {
                    Some(format!("Stopped by {signal}"))
                }
                _ => status
                    .code()
                    .filter(|code| [129, 130, 143].contains(code))
                    .map(|code| format!("Stopped by exit code {code}")),
            });
        let fallback = if let Some(reason) = outside {
            TurnOutcome::Interrupted { reason }
        } else {
            TurnOutcome::Failed {
                message: failure.unwrap_or_else(|| {
                    crash_text(observed.and_then(|status| status.code()), &lines, None)
                }),
            }
        };
        let outcome = codec.end_outcome(fallback);
        let crash = match &outcome {
            TurnOutcome::Failed { message } => Some(message.clone()),
            _ => None,
        };
        let _ = emit(
            out,
            thread,
            epoch,
            ProviderEvent::Turn {
                turn: active.id(),
                event: TurnEvent::TurnEnded {
                    outcome,
                    park: true,
                },
            },
        )
        .await;
        crash
    } else {
        failure
    };
    let status = process.child.stop().await.ok();
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut process.recorder {
        let _ = recorder.exit(
            status
                .as_ref()
                .and_then(|status| status.code())
                .unwrap_or(-1),
        );
    }
    if consumer_gone {
        return Completion::ConsumerGone;
    }
    if missing {
        return Completion::MissingSession;
    }
    let crash = if crash.is_none()
        && codec.active.is_none()
        && (!ready || status.is_some_and(|status| !status.success()))
    {
        Some(crash_text(
            status.and_then(|status| status.code()),
            &lines,
            None,
        ))
    } else {
        crash
    };
    Completion::Exited(crash)
}

pub fn crash_text(code: Option<i32>, lines: &[String], token: Option<&str>) -> String {
    let text = format!(
        "claude exited unexpectedly (exit code {})",
        code.map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
    with_stderr(&text, lines, token)
}
fn with_stderr(text: &str, lines: &[String], token: Option<&str>) -> String {
    let mut text = record::redact(text, token);
    for line in lines.iter().skip(lines.len().saturating_sub(8)) {
        text.push('\n');
        text.push_str(
            &record::redact(line, token)
                .chars()
                .take(500)
                .collect::<String>(),
        );
    }
    text
}

async fn drain_stderr(
    stderr: &mut tokio::task::JoinHandle<()>,
    wait: impl std::future::Future<Output = std::io::Result<std::process::ExitStatus>>,
    mut observed: Option<std::process::ExitStatus>,
    stdout_closed: bool,
) -> Option<std::process::ExitStatus> {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    let mut stderr_done = false;
    let mut wait_failed = false;
    tokio::pin!(wait);
    loop {
        if stderr_done && (!stdout_closed || observed.is_some()) {
            break;
        }
        tokio::select! {
            _ = &mut *stderr, if !stderr_done => { stderr_done = true; }
            _ = tokio::time::sleep_until(deadline) => break,
            status = &mut wait, if stdout_closed && observed.is_none() && !wait_failed => {
                match status {
                    Ok(status) => observed = Some(status),
                    Err(error) => {
                        tracing::warn!(%error, "could not wait for Claude exit");
                        wait_failed = true;
                    }
                }
            }
        }
    }
    if !stderr_done {
        stderr.abort();
        let _ = stderr.await;
    }
    observed
}

fn discard_stderr(child: &mut ProcessGroup) -> tokio::task::JoinHandle<()> {
    let mut lines = ByteLines::new(child.stderr().expect("piped stderr"));
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next().await {} })
}

pub(super) fn check_cancelled(cancelled: &mut oneshot::Receiver<()>) -> Result<(), ProviderError> {
    match cancelled.try_recv() {
        Err(oneshot::error::TryRecvError::Empty) => Ok(()),
        Ok(()) | Err(oneshot::error::TryRecvError::Closed) => Err(ProviderError::Cancelled),
    }
}
pub async fn probe(
    env: &ChildEnv,
    cwd: &std::path::Path,
    mut cancelled: tokio::sync::oneshot::Receiver<()>,
) -> Result<super::Probe, ProviderError> {
    let binary = env.resolve(ProviderKind::Claude)?;
    #[cfg(debug_assertions)]
    let fixture = env.fixtures.as_ref().map(|root| root.join("probe.jsonl"));
    #[cfg(not(debug_assertions))]
    let fixture: Option<std::path::PathBuf> = None;
    let version = if let Some(fixture) = &fixture {
        crate::fixture::load(fixture)?
            .into_iter()
            .find_map(|record| match record {
                crate::fixture::Record::TranscriptStart { cli_version, .. } => Some(cli_version),
                _ => None,
            })
            .unwrap_or_default()
    } else {
        check_cancelled(&mut cancelled)?;
        let mut child = env.spawn(&binary, &["--version".into()], cwd, None)?;
        let mut stdout = ByteLines::new(child.stdout().expect("piped version stdout"));
        let stderr = discard_stderr(&mut child);
        let result = tokio::select! {
            _ = &mut cancelled => Err(ProviderError::Cancelled),
            result = async {
                match stdout.next().await? {
                    Some(Line::Text(line)) => Ok(line),
                    Some(Line::Oversize(_)) => Err(ProviderError::Protocol("Claude version exceeded 4 MiB".into())),
                    None => Ok(String::new()),
                }
            } => result,
        };
        let stopped = child.stop().await;
        stderr.abort();
        let _ = stderr.await;
        stopped?;
        result?
    };
    let version = version
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_owned();
    #[cfg(debug_assertions)]
    let mut recorder = env
        .record
        .as_ref()
        .map(|root| {
            let path = root.join("probe.jsonl");
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            record::Recorder::create(&path, "claude", &version, cwd, env.home.as_deref(), None)
        })
        .transpose()?;
    check_cancelled(&mut cancelled)?;
    let mut child = env.spawn(&binary, &claude::args(None), cwd, fixture.as_deref())?;
    let mut lines = ByteLines::new(child.stdout().expect("piped stdout"));
    let stderr = discard_stderr(&mut child);
    let mut stdin = child.stdin().expect("piped stdin");
    let result = tokio::select! {
        _ = &mut cancelled => Err(ProviderError::Cancelled),
        result = async {
            let initialize = claude::initialize();
            #[cfg(debug_assertions)]
            if let Some(recorder) = &mut recorder {
                recorder.line(true, initialize.clone())?;
            }
            stdin.write_all(format!("{initialize}\n").as_bytes()).await?;
            stdin.flush().await?;
            while let Some(line) = lines.next().await? {
                let Line::Text(line) = line else {
                    return Err(ProviderError::Protocol("Claude initialize exceeded 4 MiB".into()));
                };
                let wire: serde_json::Value = serde_json::from_str(&line)?;
                #[cfg(debug_assertions)]
                if let Some(recorder) = &mut recorder { recorder.line(false, wire.clone())?; }
                if let Some(reply) = claude::probe_value(wire)? {
                    return Ok(super::Probe {
                        provider: ProviderKind::Claude,
                        path: Some(binary.clone()),
                        version: (!version.is_empty()).then_some(version),
                        readiness: if super::signed_out_account(reply.account.as_ref()) {
                            super::Readiness::SignedOut
                        } else { super::Readiness::Ready },
                        models: reply.models,
                        account: reply.account,
                    });
                }
            }
            Err(ProviderError::Protocol("Claude initialize ended without a reply".into()))
        } => result,
    };
    drop(stdin);
    let status = child.stop().await;
    stderr.abort();
    let _ = stderr.await;
    let status = status?;
    #[cfg(debug_assertions)]
    let code = status.code().unwrap_or(-1);
    #[cfg(not(debug_assertions))]
    let _ = status;
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut recorder {
        recorder.exit(code)?;
    }
    result
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn blocked_interrupt_is_checked_at_its_first_byte() {
        use tokio::io::AsyncReadExt;
        for stale in [false, true] {
            let first = TurnId::new();
            let active = Mutex::new(Some(first));
            let (mut stdin, mut reader) = tokio::io::duplex(1);
            stdin.write_all(b"x").await.unwrap();
            let mut write = Box::pin(write_interrupt(&mut stdin, b"y", first, &active));
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(write.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            if stale {
                *active.lock().unwrap() = Some(TurnId::new());
            }
            let mut byte = [0];
            reader.read_exact(&mut byte).await.unwrap();
            assert_eq!(&byte, b"x");
            write.await.unwrap();
            drop(stdin);
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await.unwrap();
            assert_eq!(bytes, if stale { vec![] } else { b"y".to_vec() });
        }
    }
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn stderr_wait_error_keeps_drain_deadline() {
        let tail = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let output = tail.clone();
        let mut stderr = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            output.lock().unwrap().push("late diagnostic");
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let start = tokio::time::Instant::now();
        let wait = async { Err(std::io::Error::other("wait failed")) };
        let observed = drain_stderr(&mut stderr, wait, None, true).await;
        assert_eq!(start.elapsed(), Duration::from_millis(500));
        assert_eq!(*tail.lock().unwrap(), ["late diagnostic"]);
        assert!(observed.is_none());
        assert!(stderr.is_finished());
    }

    #[tokio::test(start_paused = true)]
    async fn natural_exit_after_stdout_eof_wakes_without_a_timer() {
        let env = ChildEnv::load();
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "read initialize; read prompt; exec 1>&- 2>&-; sleep 0.2; exit 130".into(),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.pid();
        let process = Process {
            child,
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let (sender, control) = mpsc::unbounded_channel();
        let mut control = SessionControls::new(control, Default::default());
        sender
            .send(SessionControl::StartTurn {
                turn: TurnId::new(),
                text: "reply".into(),
            })
            .unwrap();
        let (out, mut events) = mpsc::channel(256);
        let task = tokio::spawn(async move {
            run_one(
                process,
                &mut control,
                &out,
                ThreadId::new(),
                1,
                false,
                std::path::Path::new("/tmp"),
            )
            .await
        });
        let end = std::time::Instant::now() + Duration::from_secs(5);
        let ended = loop {
            if let Ok(event) = events.try_recv() {
                break Some(event);
            }
            if std::time::Instant::now() >= end {
                break None;
            }
            tokio::task::yield_now().await;
        };
        tokio::time::resume();
        task.await.unwrap();
        assert!(!platform::group_exists(pid).unwrap());
        let EngineMsg::Provider {
            event:
                ProviderEvent::Turn {
                    event: TurnEvent::TurnEnded { outcome, .. },
                    ..
                },
            ..
        } = ended.expect("natural exit must wake the session without advancing its clock")
        else {
            panic!("missing end")
        };
        assert_eq!(
            outcome,
            TurnOutcome::Interrupted {
                reason: "Stopped by exit code 130".into()
            }
        );
    }
    #[tokio::test]
    async fn exit_by_signal_or_code_classifies() {
        for (script, interrupted, expected) in [
            ("exit 129", true, "Stopped by exit code 129"),
            ("exit 130", true, "Stopped by exit code 130"),
            ("exit 143", true, "Stopped by exit code 143"),
            (
                "exit 137",
                false,
                "claude exited unexpectedly (exit code 137)",
            ),
            ("kill -TERM $$", true, "Stopped by SIGTERM"),
            (
                "kill -KILL $$",
                false,
                "claude exited unexpectedly (exit code unknown)",
            ),
        ] {
            let env = ChildEnv::load();
            let child = env
                .spawn(
                    std::path::Path::new("/bin/sh"),
                    &[
                        "-c".into(),
                        format!("read initialize; read prompt; {script}"),
                    ],
                    std::path::Path::new("/tmp"),
                    None,
                )
                .unwrap();
            let pid = child.pid();
            let process = Process {
                child,
                native: NativeSessionId("fixture-session".into()),
                #[cfg(debug_assertions)]
                recorder: None,
            };
            let (sender, control) = mpsc::unbounded_channel();
            let mut control = SessionControls::new(control, Default::default());
            sender
                .send(SessionControl::StartTurn {
                    turn: TurnId::new(),
                    text: "reply".into(),
                })
                .unwrap();
            let (out, mut events) = mpsc::channel(256);
            run_one(
                process,
                &mut control,
                &out,
                ThreadId::new(),
                1,
                false,
                std::path::Path::new("/tmp"),
            )
            .await;
            assert!(!platform::group_exists(pid).unwrap());
            let EngineMsg::Provider {
                event:
                    ProviderEvent::Turn {
                        event: TurnEvent::TurnEnded { outcome, .. },
                        ..
                    },
                ..
            } = events.try_recv().unwrap()
            else {
                panic!("missing end")
            };
            assert_eq!(
                outcome,
                if interrupted {
                    TurnOutcome::Interrupted {
                        reason: expected.into(),
                    }
                } else {
                    TurnOutcome::Failed {
                        message: expected.into(),
                    }
                },
                "{script}"
            );
        }
    }
    #[tokio::test]
    async fn exit_after_stdout_eof_uses_natural_status() {
        let script = "exec 1>&- 2>&-; sleep 0.1; exit 130";
        let env = ChildEnv::load();
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    format!("read initialize; read prompt; {script}"),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.pid();
        let process = Process {
            child,
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let (sender, control) = mpsc::unbounded_channel();
        let mut control = SessionControls::new(control, Default::default());
        sender
            .send(SessionControl::StartTurn {
                turn: TurnId::new(),
                text: "reply".into(),
            })
            .unwrap();
        let (out, mut events) = mpsc::channel(256);
        run_one(
            process,
            &mut control,
            &out,
            ThreadId::new(),
            1,
            false,
            std::path::Path::new("/tmp"),
        )
        .await;
        assert!(!platform::group_exists(pid).unwrap());
        let EngineMsg::Provider {
            event:
                ProviderEvent::Turn {
                    event: TurnEvent::TurnEnded { outcome, .. },
                    ..
                },
            ..
        } = events.try_recv().unwrap()
        else {
            panic!("missing end")
        };
        assert_eq!(
            outcome,
            TurnOutcome::Interrupted {
                reason: "Stopped by exit code 130".into(),
            }
        );
    }

    #[tokio::test]
    async fn stdout_eof_synthesizes_end_without_extra_grace() {
        let script = "trap '' TERM; exec 1>&- 2>&-; sleep 30";
        let env = ChildEnv::load();
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    format!("read initialize; read prompt; {script}"),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.pid();
        let process = Process {
            child,
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let (sender, control) = mpsc::unbounded_channel();
        let mut control = SessionControls::new(control, Default::default());
        sender
            .send(SessionControl::StartTurn {
                turn: TurnId::new(),
                text: "reply".into(),
            })
            .unwrap();
        let (out, mut events) = mpsc::channel(256);
        let start = tokio::time::Instant::now();
        let task = tokio::spawn(async move {
            run_one(
                process,
                &mut control,
                &out,
                ThreadId::new(),
                1,
                false,
                std::path::Path::new("/tmp"),
            )
            .await
        });
        let ended = tokio::time::timeout(Duration::from_secs(9), events.recv())
            .await
            .unwrap()
            .unwrap();
        let elapsed = start.elapsed();
        task.await.unwrap();
        assert!(!platform::group_exists(pid).unwrap());
        assert!(
            elapsed < Duration::from_secs(1),
            "EOF observation added a grace period: {elapsed:?}"
        );
        let EngineMsg::Provider {
            event:
                ProviderEvent::Turn {
                    event: TurnEvent::TurnEnded { outcome, .. },
                    ..
                },
            ..
        } = ended
        else {
            panic!("missing end")
        };
        assert_eq!(
            outcome,
            TurnOutcome::Failed {
                message: "claude exited unexpectedly (exit code unknown)".into(),
            }
        );
    }

    #[tokio::test]
    async fn cleanup_deadline_delivers_failed_and_exited_when_group_stays_alive() {
        let env = ChildEnv::load();
        let cwd = std::path::PathBuf::from("/tmp");
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "read initialize; read prompt; printf 'not-json\\n'".into(),
                ],
                &cwd,
                None,
            )
            .unwrap();
        let mut group = child;
        let pid = group.pid();
        group.check = |_, _| Ok(true);
        let process = Process {
            child: group,
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let (out, mut events) = mpsc::channel(256);
        let handle = start_session(
            process,
            SessionSpec {
                thread,
                cwd,
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,
                import: false,
            },
            env,
            out,
            1,
        );
        handle
            .control
            .send(SessionControl::StartTurn {
                turn,
                text: "reply".into(),
            })
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let failed = events.recv().await.unwrap();
            let exited = events.recv().await.unwrap();
            (failed, exited)
        })
        .await;
        assert!(!platform::group_exists(pid).unwrap());
        let (failed, exited) = result.expect("cleanup must return after the two one-second waits");
        let expected = "Claude protocol: expected ident at line 1 column 2";
        assert!(matches!(failed, EngineMsg::Provider {
            event: ProviderEvent::Turn {
                turn: Some(id), event: TurnEvent::TurnEnded { outcome: TurnOutcome::Failed { message }, .. }
            }, ..
        } if id == turn && message == expected));
        assert!(matches!(exited, EngineMsg::Provider {
            event: ProviderEvent::Session(SessionEvent::Exited { crash: Some(text) }), ..
        } if text == expected));
    }
    #[tokio::test]
    async fn cleanup_keeps_the_first_check_error_after_reaping() {
        let env = ChildEnv::load();
        let mut child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "trap '' TERM; echo ready; /bin/sleep 30".into(),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let mut output = BufReader::new(child.stdout().unwrap()).lines();
        assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
        child.check = |child, _| {
            let text = if child.try_wait()?.is_none() {
                "first check failed"
            } else {
                "post-reap check failed"
            };
            Err(std::io::Error::other(text))
        };
        let result = child.stop().await;
        assert_eq!(result.unwrap_err().to_string(), "first check failed");
    }
    #[tokio::test]
    async fn session_panic_waits_for_group_cleanup_before_exited() {
        let env = ChildEnv::load();
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "trap '' TERM; /bin/sleep 30 & echo ready; wait".into(),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let mut group = child;
        let pid = group.pid();
        let mut output = BufReader::new(group.stdout().unwrap()).lines();
        assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
        let (cleanup, reaped) = mpsc::unbounded_channel();
        group.watch(&cleanup);
        drop(cleanup);
        let task = tokio::spawn(async move {
            let _group = group;
            panic!("fixture panic");
        });
        let (out, mut events) = mpsc::channel(256);
        supervise(task, reaped, out, ThreadId::new(), 1).await;
        let alive = platform::group_exists(pid).unwrap();
        if alive {
            let _ = platform::signal_group(pid, Signal::Kill);
        }
        assert!(!alive, "Exited arrived before the owned group ended");
        assert!(matches!(
            events.try_recv().unwrap(),
            EngineMsg::Provider {
                event: ProviderEvent::Session(SessionEvent::Exited { crash: Some(text) }), ..
            } if text.contains("Claude session task failed:") && text.contains("fixture panic")
        ));
    }
    #[tokio::test]
    async fn decode_failure_reports_bounded_redacted_stderr_tail() {
        let env = ChildEnv::load();
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    concat!(
                        "read initialize; read prompt; ",
                        "printf 'discarded\\n' >&2; ",
                        "printf 'discarded too\\nline 1\\nline 2\\nline 3\\nline 4\\nline 5\\n' >&2; ",
                        "printf 'password=fixture-secret\\nline 7\\nlast diagnostic\\n' >&2; ",
                        "printf 'not-json\\n'"
                    )
                    .into(),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.pid();
        let process = Process {
            child,
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let (sender, control) = mpsc::unbounded_channel();
        let mut control = SessionControls::new(control, Default::default());
        let turn = TurnId::new();
        sender
            .send(SessionControl::StartTurn {
                turn,
                text: "reply".into(),
            })
            .unwrap();
        let (out, mut events) = mpsc::channel(256);
        let completion = run_one(
            process,
            &mut control,
            &out,
            ThreadId::new(),
            1,
            false,
            std::path::Path::new("/tmp"),
        )
        .await;
        let expected = concat!(
            "Claude protocol: expected ident at line 1 column 2\n",
            "line 1\nline 2\nline 3\nline 4\nline 5\n",
            "password=[redacted]\nline 7\nlast diagnostic"
        );
        let Completion::Exited(Some(text)) = completion else {
            panic!("missing failure")
        };
        assert_eq!(text, expected);
        assert!(!platform::group_exists(pid).unwrap());
        assert!(matches!(
            events.try_recv().unwrap(),
            EngineMsg::Provider {
                event: ProviderEvent::Turn {
                    turn: Some(id),
                    event: TurnEvent::TurnEnded {
                        outcome: TurnOutcome::Failed { message }, ..
                    }
                }, ..
            } if id == turn && message == expected
        ));
    }
    #[tokio::test]
    async fn process_check_error_still_kills_and_reaps_group() {
        let env = ChildEnv::load();
        let mut child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    "trap '' TERM; /bin/sleep 30 & echo ready; wait".into(),
                ],
                std::path::Path::new("/tmp"),
                None,
            )
            .unwrap();
        let pid = child.pid();
        let mut output = BufReader::new(child.stdout().unwrap()).lines();
        assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
        child.check = |_, _| Err(std::io::Error::other("check failed"));
        let result = child.stop().await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while platform::group_exists(pid).unwrap() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let alive = platform::group_exists(pid).unwrap();
        if alive {
            let _ = platform::signal_group(pid, Signal::Kill);
        }
        assert_eq!(result.unwrap_err().to_string(), "check failed");
        assert!(!alive, "cleanup left an owned descendant alive");
    }
    #[test]
    fn crash_text_bounds_and_redacts() {
        assert_eq!(
            crash_text(None, &[], None),
            "claude exited unexpectedly (exit code unknown)"
        );
        let token = "test-session-credential";
        let mut lines = vec![
            "discarded".into(),
            "discarded too".into(),
            token.into(),
            "bEaReR abc123".into(),
            "API_KEY=abc123 secret: abc123 \"password\": \"abc123\" auth: abc123".into(),
            format!("sk-{} xai-{}", "a".repeat(30), "b".repeat(30)),
            "https://user:abc123@example.invalid".into(),
        ];
        lines.extend(["é".repeat(501), "end".into(), "last".into()]);
        let text = crash_text(Some(7), &lines, Some(token));
        assert_eq!(text.lines().count(), 9);
        assert_eq!(
            text.lines().next().unwrap(),
            "claude exited unexpectedly (exit code 7)"
        );
        assert!(text.contains("bEaReR [redacted]"));
        assert!(text.contains("API_KEY=[redacted]"));
        assert!(text.contains("https://user:[redacted]@example.invalid"));
        assert_eq!(text.lines().nth(6).unwrap().chars().count(), 500);
        assert!(!text.contains("abc123") && !text.contains(token) && !text.contains("discarded"));
        assert!(!record::secret_shapes(&text.replace("[redacted]", "<any>")));
    }
    #[tokio::test]
    async fn stopped_group_cleanup_keeps_five_second_turn_boundary() {
        let env = ChildEnv::load();
        let cwd = std::path::PathBuf::from("/tmp");
        let child = env
            .spawn(
                std::path::Path::new("/bin/sh"),
                &[
                    "-c".into(),
                    concat!(
                        "trap '' TERM; read initialize; read prompt; printf '%s\\n' ",
                        "'{\"type\":\"assistant\",\"message\":{\"id\":\"probe-message\",",
                        "\"content\":[{\"type\":\"text\",\"text\":\"ready\"}]}}'; ",
                        "read interrupt; /bin/sleep 30"
                    )
                    .into(),
                ],
                &cwd,
                None,
            )
            .unwrap();
        let mut group = child;
        let pid = group.pid();
        group.check = |_, _| Ok(true);
        let process = Process {
            child: group,
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let thread = ThreadId::new();
        let turn = TurnId::new();
        let (out, mut events) = mpsc::channel(256);
        let handle = start_session(
            process,
            SessionSpec {
                thread,
                cwd,
                config: ThreadConfig {
                    model: None,
                    effort: None,
                    mode: PermissionMode::AskFirst,
                },
                native: None,
                import: false,
            },
            env,
            out,
            1,
        );
        handle
            .control
            .send(SessionControl::StartTurn {
                turn,
                text: "reply".into(),
            })
            .unwrap();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if matches!(
                    events.recv().await.unwrap(),
                    EngineMsg::Provider {
                        event: ProviderEvent::Turn {
                            event: TurnEvent::TextDelta { .. },
                            ..
                        },
                        ..
                    }
                ) {
                    break;
                }
            }
        })
        .await
        .expect("owned child acknowledged prompt");
        let at = tokio::time::Instant::now();
        handle
            .control
            .send(SessionControl::Interrupt {
                turn,
                reason: StopReason::User,
            })
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(9), async {
            loop {
                if matches!(
                    events.recv().await.unwrap(),
                    EngineMsg::Provider {
                        event: ProviderEvent::Turn {
                            event: TurnEvent::TurnEnded {
                                outcome: TurnOutcome::Interrupted { .. },
                                park: true,
                            },
                            ..
                        },
                        ..
                    }
                ) {
                    break;
                }
            }
            let elapsed = at.elapsed();
            loop {
                if matches!(
                    events.recv().await.unwrap(),
                    EngineMsg::Provider {
                        event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                        ..
                    }
                ) {
                    break;
                }
            }
            elapsed
        })
        .await;
        assert!(
            !platform::group_exists(pid).unwrap(),
            "owned child group survived"
        );
        let elapsed = result.expect("owned group cleanup finished");
        println!("synthesized interrupted end after {elapsed:?}");
        assert!(
            elapsed <= Duration::from_secs(5),
            "Stop drain exceeded five seconds: {elapsed:?}"
        );
    }
    async fn assert_stand_in_starts(
        env: &ChildEnv,
        cwd: &std::path::Path,
        marker: &std::path::Path,
    ) {
        let child = env
            .spawn(env.claude_bin.as_ref().unwrap(), &[], cwd, None)
            .unwrap();
        let process = child;
        let started = tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        process.stop().await.unwrap();
        started.unwrap();
        assert!(marker.exists(), "uncancelled stand-in did not start");
        assert!(env.groups.lock().unwrap().is_empty());
        std::fs::remove_file(marker).unwrap();
    }

    #[tokio::test]
    async fn shutdown_during_missing_resume_never_launches_replacement() {
        for during_cleanup in [false, true] {
            for shutdown in [true, false] {
                let cwd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                    "../../target/quit-missing-resume-{}",
                    ThreadId::new()
                ));
                std::fs::create_dir_all(&cwd).unwrap();
                let _scratch = crate::tests::Scratch(cwd.clone());
                let marker = cwd.join("replacement-spawned");
                let cleanup = cwd.join("cleanup-started");
                let binary = cwd.join("owned-replacement-stand-in");
                std::fs::write(
                    &binary,
                    "#!/bin/sh\nprintf launched > replacement-spawned\nread initialize\n",
                )
                .unwrap();
                assert!(
                    std::process::Command::new("/bin/chmod")
                        .arg("700")
                        .arg(&binary)
                        .status()
                        .unwrap()
                        .success()
                );
                let mut env = ChildEnv::load();
                env.groups = Default::default();
                env.claude_bin = Some(binary);
                env.fixtures = None;
                env.record = None;
                assert_stand_in_starts(&env, &cwd, &marker).await;
                let script = if during_cleanup {
                    concat!(
                        "trap 'printf cleanup > cleanup-started' TERM; ",
                        "printf 'No conversation found with session ID: missing-native\\n' >&2; ",
                        "read initialize; exec 1>&- 2>&-; while :; do /bin/sleep 1; done"
                    )
                } else {
                    concat!(
                        "printf 'No conversation found with session ID: missing-native\\n' >&2; ",
                        "read initialize; read until_stdin_closes; exit 1"
                    )
                };
                let child = env
                    .spawn(
                        std::path::Path::new("/bin/sh"),
                        &["-c".into(), script.into()],
                        &cwd,
                        None,
                    )
                    .unwrap();
                let pid = child.pid();
                let process = Process {
                    child,
                    native: NativeSessionId("missing-native".into()),
                    recorder: None,
                };
                let thread = ThreadId::new();
                let (out, mut events) = mpsc::channel(256);
                let handle = start_session(
                    process,
                    SessionSpec {
                        thread,
                        cwd,
                        config: ThreadConfig {
                            model: None,
                            effort: None,
                            mode: PermissionMode::AskFirst,
                        },
                        native: Some(NativeSessionId("missing-native".into())),
                        import: false,
                    },
                    env.clone(),
                    out,
                    1,
                );
                if during_cleanup {
                    tokio::time::timeout(Duration::from_secs(3), async {
                        while !cleanup.exists() {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .unwrap();
                }
                if shutdown {
                    handle.control.send(SessionControl::Shutdown).unwrap();
                } else {
                    drop(handle);
                }
                let exited = tokio::time::timeout(Duration::from_secs(5), async {
                    loop {
                        let event = events.recv().await.unwrap();
                        if matches!(
                            event,
                            EngineMsg::Provider {
                                event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                                ..
                            }
                        ) {
                            break event;
                        }
                    }
                })
                .await;
                assert!(
                    !platform::group_exists(pid).unwrap(),
                    "owned resumed group was not reaped"
                );
                assert!(
                    env.groups.lock().unwrap().is_empty(),
                    "owned replacement was not reaped"
                );
                assert!(
                    !marker.exists(),
                    "cancellation launched a replacement: shutdown={shutdown} during_cleanup={during_cleanup}"
                );
                assert!(matches!(
                    exited.unwrap(),
                    EngineMsg::Provider {
                        event: ProviderEvent::Session(SessionEvent::Exited { crash: None }),
                        ..
                    }
                ));
            }
        }
    }

    #[tokio::test]
    async fn shutdown_before_session_launch_starts_no_provider() {
        for shutdown in [true, false] {
            let cwd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
                "../../target/quit-before-launch-{}",
                ThreadId::new()
            ));
            std::fs::create_dir_all(&cwd).unwrap();
            let _scratch = crate::tests::Scratch(cwd.clone());
            let marker = cwd.join("provider-spawned");
            let binary = cwd.join("owned-session-stand-in");
            std::fs::write(
                &binary,
                "#!/bin/sh\nprintf launched > provider-spawned\nread initialize\n",
            )
            .unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&binary)
                    .status()
                    .unwrap()
                    .success()
            );
            let mut env = ChildEnv::load();
            env.groups = Default::default();
            env.claude_bin = Some(binary);
            env.fixtures = None;
            env.record = None;
            assert_stand_in_starts(&env, &cwd, &marker).await;
            let (out, mut events) = mpsc::channel(256);
            let handle = spawn(
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd,
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::AskFirst,
                    },
                    native: None,
                    import: false,
                },
                env.clone(),
                out,
                1,
            )
            .unwrap();
            handle
                .control
                .send(SessionControl::StartTurn {
                    turn: TurnId::new(),
                    text: "queued before shutdown".into(),
                })
                .unwrap();
            if shutdown {
                handle.control.send(SessionControl::Shutdown).unwrap();
            } else {
                drop(handle);
            }
            let exited = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let event = events.recv().await.unwrap();
                    if matches!(
                        event,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                            ..
                        }
                    ) {
                        break event;
                    }
                }
            })
            .await;
            assert!(
                env.groups.lock().unwrap().is_empty(),
                "owned initial group was not reaped"
            );
            assert!(
                !marker.exists(),
                "cancelled initial launch started a provider: shutdown={shutdown}"
            );
            assert!(matches!(
                exited.unwrap(),
                EngineMsg::Provider {
                    event: ProviderEvent::Session(SessionEvent::Exited { crash: None }),
                    ..
                }
            ));
        }
    }
    #[tokio::test]
    async fn pending_shutdown_preempts_queued_turn_on_ready_session() {
        for cancellation in ["shutdown", "closed", "engine", "none"] {
            let cwd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../target/session-close-{}", ThreadId::new()));
            std::fs::create_dir_all(&cwd).unwrap();
            let _scratch = crate::tests::Scratch(cwd.clone());
            let binary = cwd.join("owned-session-stand-in");
            std::fs::write(
                &binary,
                r#"#!/bin/sh
read initialize
printf '%s\n' '{"type":"control_response","response":{"subtype":"success",'\
'"request_id":"veyra-initialize","response":{}}}'
while read line; do
  case "$line" in *'"type":"user"'*) printf started > turn-started;; esac
done
"#,
            )
            .unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&binary)
                    .status()
                    .unwrap()
                    .success()
            );
            let mut env = ChildEnv::load();
            env.groups = Default::default();
            env.claude_bin = Some(binary);
            env.fixtures = None;
            env.record = Some(cwd.join("rec"));
            let (out, mut events) = mpsc::channel(256);
            let handle = spawn(
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: cwd.clone(),
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::AskFirst,
                    },
                    native: None,
                    import: false,
                },
                env.clone(),
                out,
                1,
            )
            .unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(message) = events.recv().await {
                    if matches!(
                        message,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Ready { .. }),
                            ..
                        }
                    ) {
                        return;
                    }
                }
                panic!("session ended before ready");
            })
            .await
            .unwrap();
            handle
                .control
                .send(SessionControl::StartTurn {
                    turn: TurnId::new(),
                    text: "already cancelled".into(),
                })
                .unwrap();
            match cancellation {
                "shutdown" => handle.control.send(SessionControl::Shutdown).unwrap(),
                "closed" => drop(handle),
                "engine" => env.closing.close(),
                "none" => {
                    tokio::time::timeout(Duration::from_secs(3), async {
                        while !cwd.join("turn-started").exists() {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .unwrap();
                    handle.control.send(SessionControl::Shutdown).unwrap();
                }
                _ => unreachable!(),
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(message) = events.recv().await {
                    if matches!(
                        message,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                            ..
                        }
                    ) {
                        return;
                    }
                }
                panic!("session ended without exit");
            })
            .await
            .unwrap();
            assert!(
                env.groups.lock().unwrap().is_empty(),
                "owned session group survived cleanup"
            );
            let capture = std::fs::read_to_string(cwd.join("rec/t1-s1.jsonl")).unwrap();
            let users = capture
                .lines()
                .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
                .filter(|record| {
                    record["type"] == "expect_outbound" && record["line"]["type"] == "user"
                })
                .count();
            assert_eq!(
                users,
                usize::from(cancellation == "none"),
                "cancelled session wrote a new user prompt: {cancellation}"
            );
            assert!(
                cwd.join("turn-started").exists() == (cancellation == "none"),
                "owned stand-in received a user turn after cancellation: {cancellation}"
            );
        }
    }
    #[tokio::test]
    async fn shutdown_after_admission_before_stdin_write_sends_no_prompt() {
        for cancellation in ["none", "writer"] {
            let cwd = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../target/session-close-{}", ThreadId::new()));
            std::fs::create_dir_all(&cwd).unwrap();
            let _scratch = crate::tests::Scratch(cwd.clone());
            let binary = cwd.join("owned-session-stand-in");
            std::fs::write(
                &binary,
                r#"#!/bin/sh
read initialize
printf '%s\n' '{"type":"control_response","response":{"subtype":"success",'\
'"request_id":"veyra-initialize","response":{}}}'
while read line; do
  case "$line" in *'"type":"user"'*) printf started > turn-started;; esac
done
"#,
            )
            .unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&binary)
                    .status()
                    .unwrap()
                    .success()
            );
            let mut env = ChildEnv::load();
            env.groups = Default::default();
            env.claude_bin = Some(binary);
            env.fixtures = None;
            env.record = Some(cwd.join("rec"));
            let (out, mut events) = mpsc::channel(256);
            let handle = spawn(
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: cwd.clone(),
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::AskFirst,
                    },
                    native: None,
                    import: false,
                },
                env.clone(),
                out,
                1,
            )
            .unwrap();
            tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(message) = events.recv().await {
                    if matches!(
                        message,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Ready { .. }),
                            ..
                        }
                    ) {
                        return;
                    }
                }
                panic!("session ended before ready");
            })
            .await
            .unwrap();
            if cancellation == "writer" {
                env.close_before_user_write.store(true, Ordering::Release);
            }
            handle
                .control
                .send(SessionControl::StartTurn {
                    turn: TurnId::new(),
                    text: "already cancelled".into(),
                })
                .unwrap();
            match cancellation {
                "shutdown" => handle.control.send(SessionControl::Shutdown).unwrap(),
                "closed" => drop(handle),
                "engine" => env.closing.close(),
                "none" | "writer" => {
                    tokio::time::timeout(Duration::from_secs(3), async {
                        while if cancellation == "writer" {
                            !env.closing.is_closing()
                        } else {
                            !cwd.join("turn-started").exists()
                        } {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    })
                    .await
                    .unwrap();
                    handle.control.send(SessionControl::Shutdown).unwrap();
                }
                _ => unreachable!(),
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(message) = events.recv().await {
                    if matches!(
                        message,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                            ..
                        }
                    ) {
                        return;
                    }
                }
                panic!("session ended without exit");
            })
            .await
            .unwrap();
            assert!(
                env.groups.lock().unwrap().is_empty(),
                "owned session group survived cleanup"
            );

            if cancellation == "writer" {
                assert!(env.closing.is_closing());
                assert!(
                    !cwd.join("turn-started").exists(),
                    "dedicated stdin writer delivered a user prompt after closing was set; all owned groups cleaned up"
                );
            } else {
                assert!(
                    cwd.join("turn-started").exists(),
                    "positive control did not start"
                );
            }
        }
    }
}
