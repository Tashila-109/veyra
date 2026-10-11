use super::{
    ProviderError, ProviderEvent, ProviderKind, SessionControl, SessionEvent, SessionHandle,
    SessionSpec, TurnEvent,
    claude::{self, Codec, InterruptPhase},
    env::ChildEnv,
};
use crate::{
    actor::EngineMsg,
    domain::*,
    platform::{self, Signal},
    record,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader},
    process::Child,
    sync::{mpsc, oneshot},
};

struct ProcessGroup {
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
    fn new(child: Child) -> Self {
        Self {
            pid: child.id().expect("new child"),
            child: Some(child),
            reaped: None,
            groups: None,
            #[cfg(test)]
            check: process_alive,
        }
    }
    fn tracked(child: Child, env: &ChildEnv) -> Self {
        let mut group = Self::new(child);
        env.groups.lock().unwrap().insert(group.pid);
        group.groups = Some(env.groups.clone());
        group
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
    async fn stop(mut self) -> std::io::Result<std::process::ExitStatus> {
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
struct Process {
    child: ProcessGroup,
    native: NativeSessionId,
    #[cfg(debug_assertions)]
    recorder: Option<record::Recorder>,
}
impl Process {
    fn send(
        &mut self,
        writer: &mpsc::UnboundedSender<serde_json::Value>,
        line: serde_json::Value,
    ) -> Result<(), ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            recorder.line(true, line.clone())?;
        }
        writer
            .send(line)
            .map_err(|_| ProviderError::Protocol("Claude stdin writer closed".into()))
    }
    fn record_inbound(&mut self, line: serde_json::Value) -> Result<(), ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            recorder.line(false, line)?;
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
        child: ProcessGroup::tracked(child, env),
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
        let mut process = match process {
            Some(process) => process,
            None => match launch(&spec, &env, None) {
                Ok(process) => process,
                Err(error) => {
                    return emit(&out, thread, epoch, launch_failed(error)).await;
                }
            },
        };
        process.child.watch(&cleanup);
        let mut receiver = receiver;
        loop {
            match run_one(
                process,
                &mut receiver,
                &out,
                thread,
                epoch,
                spec.native.is_some(),
            )
            .await
            {
                Completion::MissingSession => {
                    spec.native = None;
                    let native = NativeSessionId(ThreadId::new().to_string());
                    match launch(&spec, &env, Some(native)) {
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
        crash: Some(record::redact(&error.to_string(), None)),
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
enum Completion {
    MissingSession,
    Exited(Option<String>),
    ConsumerGone,
}

async fn run_one(
    mut process: Process,
    control: &mut mpsc::UnboundedReceiver<SessionControl>,
    out: &mpsc::Sender<EngineMsg>,
    thread: ThreadId,
    epoch: u32,
    resume: bool,
) -> Completion {
    let pid = process.child.pid;
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
    let (writer, mut input) = mpsc::unbounded_channel::<serde_json::Value>();
    let mut write_task = tokio::spawn(async move {
        while let Some(line) = input.recv().await {
            let mut bytes = serde_json::to_vec(&line)?;
            bytes.push(b'\n');
            stdin.write_all(&bytes).await?;
            stdin.flush().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let mut codec = Codec::new(process.native.clone(), pid);
    let mut ready = false;
    let mut rejected = None;
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut process.recorder {
        recorder.bind(process.native.0.clone(), "<id:session>".into());
    }
    let mut failure = process
        .send(&writer, claude::initialize())
        .err()
        .map(|error| error.to_string());
    let mut consumer_gone = false;
    let mut writer_done = false;
    while failure.is_none() {
        tokio::select! {
            biased;
            _ = out.closed() => { consumer_gone = true; break; }
            message = control.recv() => match message {
                Some(SessionControl::StartTurn { turn, text }) => {
                    let line = codec.start(turn, text);
                    #[cfg(debug_assertions)]
                    if let Some(recorder) = &mut process.recorder {
                        recorder.bind(turn.to_string(), format!("<id:turn:{turn}>"));
                    }
                    if let Err(error) = process.send(&writer, line) {
                        failure = Some(error.to_string());
                        break;
                    }
                }
                Some(SessionControl::Shutdown) | None => {
                    for line in interrupt_lines(&mut codec, "Veyra closed while this turn ran") {
                        if let Err(error) = process.send(&writer, line) {
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
                Some(message @ (SessionControl::Interrupt | SessionControl::StorageBlocked)) => {
                    let reason = if message == SessionControl::Interrupt { "Stopped by you" }
                        else { "Stopped because the database is blocked" };
                    for line in interrupt_lines(&mut codec, reason) {
                        if let Err(error) = process.send(&writer, line) {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
                Some(SessionControl::Answer { native, reply }) => {
                    if let Some(line) = codec.answer(native, reply)
                        && let Err(error) = process.send(&writer, line)
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
                match &codec.interrupted {
                    Some(interrupt) => tokio::time::sleep_until(interrupt.phase.deadline()).await,
                    None => std::future::pending().await,
                }
            } => {
                match codec.interrupted.as_ref().map(|interrupt| &interrupt.phase) {
                    Some(InterruptPhase::Protocol(_)) => {
                        let _ = platform::signal_group(pid, Signal::Terminate);
                        codec.interrupted.as_mut().unwrap().phase =
                            InterruptPhase::Terminate(tokio::time::Instant::now() + Duration::from_secs(1));
                    }
                    Some(InterruptPhase::Terminate(_)) => { let _ = platform::signal_group(pid, Signal::Kill); break; }
                    None => {}
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
                    if let Err(error) = process.record_inbound(wire.clone()) {
                        failure = Some(error.to_string());
                        break;
                    }
                    match codec.value(wire) {
                        Ok(decoded) => {
                            for reply in decoded.replies {
                                if let Err(error) = process.send(&writer, reply) {
                                    failure = Some(error.to_string());
                                    break;
                                }
                            }
                            if failure.is_some() { break; }
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
                Ok(None) => break,
                Err(error) => { failure = Some(error.to_string()); break; }
            }
        }
    }
    if !writer_done {
        write_task.abort();
        let _ = write_task.await;
    }
    if tokio::time::timeout(Duration::from_millis(500), &mut stderr)
        .await
        .is_err()
    {
        stderr.abort();
        let _ = stderr.await;
    }
    let lines = tail.lock().unwrap().iter().cloned().collect::<Vec<_>>();
    let failure = failure
        .or(rejected)
        .map(|text| with_stderr(&text, &lines, None));
    let observed = process
        .child
        .child
        .as_mut()
        .and_then(|child| child.try_wait().ok().flatten());
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
    if resume
        && lines
            .iter()
            .any(|line| line.contains("No conversation found with session ID"))
    {
        return Completion::MissingSession;
    }
    let crash = if let Some(active) = codec.active {
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
        let outcome = if let Some(interrupt) = &codec.interrupted {
            TurnOutcome::Interrupted {
                reason: interrupt.reason.clone(),
            }
        } else if let Some(text) = codec.failure() {
            TurnOutcome::Failed {
                message: text.into(),
            }
        } else if let Some(reason) = outside {
            TurnOutcome::Interrupted { reason }
        } else {
            TurnOutcome::Failed {
                message: failure.unwrap_or_else(|| {
                    crash_text(observed.and_then(|status| status.code()), &lines, None)
                }),
            }
        };
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
    } else if let Some(failure) = failure {
        Some(failure)
    } else if !ready || status.is_some_and(|status| !status.success()) {
        Some(crash_text(
            status.and_then(|status| status.code()),
            &lines,
            None,
        ))
    } else {
        None
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

fn discard_stderr(child: &mut ProcessGroup) -> tokio::task::JoinHandle<()> {
    let mut lines = ByteLines::new(child.stderr().expect("piped stderr"));
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next().await {} })
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
        let mut child =
            ProcessGroup::tracked(env.spawn(&binary, &["--version".into()], cwd, None)?, env);
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
    let mut child = ProcessGroup::tracked(
        env.spawn(&binary, &claude::args(None), cwd, fixture.as_deref())?,
        env,
    );
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

fn interrupt_lines(codec: &mut Codec, reason: &str) -> Vec<serde_json::Value> {
    if !codec.interrupt_for(reason) {
        return Vec::new();
    }
    let mut lines = codec.cancel_requests(reason);
    lines.push(serde_json::json!({
        "type":"control_request", "request_id":"veyra-interrupt",
        "request":{"subtype":"interrupt"}
    }));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let pid = child.id().unwrap();
            let process = Process {
                child: ProcessGroup::new(child),
                native: NativeSessionId("fixture-session".into()),
                #[cfg(debug_assertions)]
                recorder: None,
            };
            let (sender, mut control) = mpsc::unbounded_channel();
            sender
                .send(SessionControl::StartTurn {
                    turn: TurnId::new(),
                    text: "reply".into(),
                })
                .unwrap();
            let (out, mut events) = mpsc::channel(256);
            run_one(process, &mut control, &out, ThreadId::new(), 1, false).await;
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
        let mut group = ProcessGroup::new(child);
        let pid = group.pid;
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
        let pid = child.id().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
        let result = end_process(child, pid, |child, _| {
            let text = if child.try_wait()?.is_none() {
                "first check failed"
            } else {
                "post-reap check failed"
            };
            Err(std::io::Error::other(text))
        })
        .await;
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
        let mut group = ProcessGroup::new(child);
        let pid = group.pid;
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
        let pid = child.id().unwrap();
        let process = Process {
            child: ProcessGroup::new(child),
            native: NativeSessionId("fixture-session".into()),
            #[cfg(debug_assertions)]
            recorder: None,
        };
        let (sender, mut control) = mpsc::unbounded_channel();
        let turn = TurnId::new();
        sender
            .send(SessionControl::StartTurn {
                turn,
                text: "reply".into(),
            })
            .unwrap();
        let (out, mut events) = mpsc::channel(256);
        let completion = run_one(process, &mut control, &out, ThreadId::new(), 1, false).await;
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
        let pid = child.id().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
        assert_eq!(output.next_line().await.unwrap().as_deref(), Some("ready"));
        let result = end_process(child, pid, |_, _| {
            Err(std::io::Error::other("check failed"))
        })
        .await;
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
}
