#[cfg(test)]
use super::codex::git::discover as codex_git_roots;
use super::{
    ActiveTurn, Decoded, Outbound, ProviderError, ProviderEvent, ProviderKind, SessionControl,
    SessionEvent, SessionHandle, SessionSpec, StopReason, TurnEvent, claude, codex, env::ChildEnv,
    jsonrpc,
};
use crate::{
    actor::EngineMsg,
    domain::*,
    platform::{self, Signal},
    record,
};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
#[cfg(all(test, debug_assertions))]
use std::sync::atomic::Ordering;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncWrite, AsyncWriteExt},
    process::Child,
    sync::{mpsc, oneshot},
};

pub(crate) use super::lines::{ByteLines, Line};
#[cfg(test)]
use tokio::io::{AsyncBufReadExt, BufReader};

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
    pub(super) fn stdout(&mut self) -> Option<tokio::process::ChildStdout> {
        self.child.as_mut()?.stdout.take()
    }
    fn stderr(&mut self) -> Option<tokio::process::ChildStderr> {
        self.child.as_mut()?.stderr.take()
    }
    pub(super) fn stdin(&mut self) -> Option<tokio::process::ChildStdin> {
        self.child.as_mut()?.stdin.take()
    }
    pub(crate) async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.as_mut().unwrap().wait().await
    }
    pub(super) fn watch(&mut self, cleanup: &mpsc::UnboundedSender<oneshot::Receiver<()>>) {
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
pub(crate) async fn write_interrupt(
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
pub(crate) async fn write(
    mut stdin: impl AsyncWrite + Unpin,
    mut input: mpsc::UnboundedReceiver<Outbound>,
    active: Arc<Mutex<Option<crate::domain::TurnId>>>,
    closing: Arc<crate::LaunchGate>,
    #[cfg(test)] close_before_prompt: Arc<std::sync::atomic::AtomicBool>,
) -> std::io::Result<()> {
    while let Some(outbound) = input.recv().await {
        let prompt = matches!(&outbound, Outbound::Prompt(_));
        let interrupt = match &outbound {
            Outbound::Interrupt { turn, .. } => Some(*turn),
            _ => None,
        };
        #[cfg(test)]
        if prompt && close_before_prompt.swap(false, std::sync::atomic::Ordering::AcqRel) {
            closing.close();
        }
        if prompt && closing.is_closing() {
            continue;
        }
        let mut bytes = outbound.into_bytes()?;
        bytes.push(b'\n');
        if let Some(turn) = interrupt {
            write_interrupt(&mut stdin, &bytes, turn, &active).await?;
        } else {
            stdin.write_all(&bytes).await?;
        }
        stdin.flush().await?;
    }
    Ok(())
}
struct Process {
    io: SessionIo,
    codec: Codec,
}
struct SessionIo {
    child: ProcessGroup,
    mcp: Option<RootMcp>,
    #[cfg(debug_assertions)]
    recorder: Option<record::Recorder>,
}
enum RootMcp {
    Claude(crate::mcp::Session),
    Codex(Arc<codex::McpConfig>),
}
impl RootMcp {
    fn sanitize(&self, value: &mut serde_json::Value, replacement: &str) {
        match self {
            Self::Claude(session) => session.hash.sanitize(value, replacement),
            Self::Codex(config) => config.sanitize(value, replacement),
        }
    }
    fn redact(&self, text: &str, replacement: &str) -> String {
        match self {
            Self::Claude(session) => session.hash.redact(text, replacement),
            Self::Codex(config) => config.redact(text, replacement),
        }
    }
}
impl SessionIo {
    fn send(
        &mut self,
        writer: &mpsc::UnboundedSender<Outbound>,
        line: Outbound,
    ) -> Result<(), ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            let mut value = line.value();
            if let Some(mcp) = &self.mcp {
                mcp.sanitize(&mut value, "<id:token>");
            }
            recorder.line(true, value)?;
        }
        writer
            .send(line)
            .map_err(|_| ProviderError::Protocol("Provider stdin writer closed".into()))
    }
    fn record_inbound(
        &mut self,
        line: serde_json::Value,
        codec: &mut Codec,
    ) -> Result<Decoded, ProviderError> {
        if let Some(RootMcp::Claude(session)) = &self.mcp {
            session.config.system_init(&line)?;
        }
        let mut decoded_line = line.clone();
        if let Some(mcp) = &self.mcp {
            mcp.sanitize(&mut decoded_line, "[redacted]");
        }
        let decoded = codec.value(decoded_line);
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            if let Ok(decoded) = &decoded
                && let Some(metadata) = &decoded.recording
            {
                recorder.metadata(metadata)?;
            }
            let mut recorded = line.clone();
            if let Some(mcp) = &self.mcp {
                mcp.sanitize(&mut recorded, "<id:token>");
            }
            recorder.line(false, recorded)?;
        }
        decoded
    }
}

async fn launch(
    provider: ProviderKind,
    spec: &SessionSpec,
    env: &ChildEnv,
    fresh: Option<NativeSessionId>,
    session_epoch: u32,
    before_spawn: impl FnOnce() -> Result<(), ProviderError>,
    cleanup: &mpsc::UnboundedSender<oneshot::Receiver<()>>,
) -> Result<Process, ProviderError> {
    let server = if spec.root { env.mcp.as_ref() } else { None };
    match provider {
        ProviderKind::Claude => {
            before_spawn()?;
            let native = fresh
                .or_else(|| spec.native.clone())
                .unwrap_or_else(|| NativeSessionId(spec.thread.to_string()));
            let mut args = claude::args(Some(spec));
            if spec.native.is_none()
                && let Some(index) = args.iter().position(|arg| arg == "--session-id")
            {
                args[index + 1] = native.0.clone();
            }
            launch_with(
                provider,
                spec,
                env,
                args,
                "unknown until system/init",
                |args, spawn| {
                    let (child, mcp) = if let Some(server) = server {
                        let (child, config, hash) =
                            server.for_spawn(spec.thread, session_epoch, |token| {
                                let config = crate::mcp::Configuration::create(
                                    server.socket.parent().unwrap(),
                                    spec.thread,
                                    &server.command,
                                    &server.socket,
                                    token,
                                )?;
                                args.extend([
                                    "--mcp-config".into(),
                                    config.path.to_string_lossy().into_owned(),
                                ]);
                                let child = spawn(args)?;
                                let pid = child.pid();
                                Ok(((child, config, crate::mcp::TokenHash(token.hash())), pid))
                            })?;
                        (
                            child,
                            Some(RootMcp::Claude(crate::mcp::Session {
                                config,
                                hash,
                                server: server.clone(),
                            })),
                        )
                    } else {
                        (spawn(args)?, None)
                    };
                    let codec =
                        Codec::Claude(Box::new(claude::Codec::new(native, child.pid(), &spec.cwd)));
                    Ok((child, mcp, codec))
                },
            )
        }
        ProviderKind::Codex => {
            let roots = if spec.config.mode == PermissionMode::AcceptEdits {
                codex::git::discover(env, &spec.cwd, cleanup).await
            } else {
                Vec::new()
            };
            before_spawn()?;
            launch_with(
                provider,
                spec,
                env,
                vec!["app-server".into()],
                "unknown until thread reply",
                |args, spawn| {
                    let child = spawn(args)?;
                    let config = server.map(|server| {
                        Arc::new(codex::McpConfig::new(
                            server.clone(),
                            spec.thread,
                            session_epoch,
                            child.pid(),
                        ))
                    });
                    let codec = Codec::Codex(Box::new(
                        codex::Codec::new(spec.clone(), child.pid(), config.clone())
                            .with_workspace_roots(roots),
                    ));
                    Ok((child, config.map(RootMcp::Codex), codec))
                },
            )
        }
        ProviderKind::Grok => Err(ProviderError::NotBuilt),
    }
}
fn launch_with(
    provider: ProviderKind,
    spec: &SessionSpec,
    env: &ChildEnv,
    mut args: Vec<String>,
    initial_version: &str,
    build: impl FnOnce(
        &mut Vec<String>,
        &mut dyn FnMut(&[String]) -> Result<ProcessGroup, ProviderError>,
    ) -> Result<(ProcessGroup, Option<RootMcp>, Codec), ProviderError>,
) -> Result<Process, ProviderError> {
    let binary = env.resolve(provider)?;
    #[cfg(debug_assertions)]
    let (n, epoch) = env.next_lane(spec.thread);
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
                provider,
                initial_version,
                &spec.cwd,
                env.home.as_deref(),
                None,
            )
        })
        .transpose()?;
    #[cfg(not(debug_assertions))]
    let _ = initial_version;
    let (child, mcp, codec) = build(&mut args, &mut |args| {
        env.spawn(&binary, args, &spec.cwd, fixture.as_deref())
    })?;
    Ok(Process {
        io: SessionIo {
            child,
            mcp,
            #[cfg(debug_assertions)]
            recorder,
        },
        codec,
    })
}
#[cfg(test)]
impl Process {
    fn test(
        child: ProcessGroup,
        native: NativeSessionId,
        mcp: Option<crate::mcp::Session>,
        #[cfg(debug_assertions)] recorder: Option<record::Recorder>,
    ) -> Self {
        let codec = Codec::Claude(Box::new(claude::Codec::new(
            native,
            child.pid(),
            std::path::Path::new("/tmp"),
        )));
        Self {
            io: SessionIo {
                child,
                mcp: mcp.map(RootMcp::Claude),
                #[cfg(debug_assertions)]
                recorder,
            },
            codec,
        }
    }
}
pub fn spawn(
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
) -> Result<SessionHandle, ProviderError> {
    spawn_provider(ProviderKind::Claude, spec, env, out, epoch)
}
pub fn spawn_provider(
    provider: ProviderKind,
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
) -> Result<SessionHandle, ProviderError> {
    if !provider.is_built() {
        return Err(ProviderError::NotBuilt);
    }
    env.resolve(provider)?;
    Ok(start_provider_task(provider, None, spec, env, out, epoch))
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
fn start_provider_task(
    provider: ProviderKind,
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
                match async {
                    receiver.drain_and_check_launch(&out)?;
                    launch(
                        provider,
                        &spec,
                        &env,
                        None,
                        epoch,
                        || receiver.drain_and_check_launch(&out),
                        &cleanup,
                    )
                    .await
                }
                .await
                {
                    Ok(process) => process,
                    Err(error) => {
                        return emit(&out, thread, epoch, launch_failed(error)).await;
                    }
                }
            }
        };
        process.io.child.watch(&cleanup);
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
                    match async {
                        receiver.drain_and_check_launch(&out)?;
                        launch(
                            provider,
                            &spec,
                            &env,
                            Some(native),
                            epoch,
                            || receiver.drain_and_check_launch(&out),
                            &cleanup,
                        )
                        .await
                    }
                    .await
                    {
                        Ok(mut next) => {
                            next.io.child.watch(&cleanup);
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
    tokio::spawn(supervise(
        task,
        reaped,
        supervisor_out,
        thread,
        epoch,
        provider,
    ));
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
    provider: ProviderKind,
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
                    &format!("{} session task failed: {error}", provider.name()),
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
    process: Process,
    control: &mut SessionControls,
    out: &mpsc::Sender<EngineMsg>,
    thread: ThreadId,
    epoch: u32,
    resume: bool,
) -> Completion {
    let Process {
        io: mut process,
        mut codec,
    } = process;
    let pid = process.child.pid();
    let profile = codec.profile();
    let provider = profile.provider;
    let stdout = process.child.stdout().expect("piped stdout");
    let mut lines = (profile.lines)(stdout);
    let mut errors = ByteLines::new(process.child.stderr().expect("piped stderr"));
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stderr_tail = tail.clone();
    let mut stderr = tokio::spawn(async move {
        while let Ok(Some(line)) = errors.next().await {
            let line = match line {
                Line::Text(line) => line,
                Line::Oversize(length) => format!("Provider stderr line skipped ({length} bytes)"),
            };
            let mut tail = stderr_tail.lock().unwrap();
            tail.push_back(line);
            while tail.len() > 8 {
                tail.pop_front();
            }
        }
    });
    let stdin = process.child.stdin().expect("piped stdin");
    let (writer, input) = mpsc::unbounded_channel::<Outbound>();
    let active_turn = Arc::new(Mutex::new(None));
    let mut write_task = tokio::spawn(write(
        stdin,
        input,
        active_turn.clone(),
        control.closing.clone(),
        #[cfg(test)]
        control.close_before_user_write.clone(),
    ));
    let mut ready = false;
    let mut rejected = None;
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut process.recorder
        && let Some(native) = codec.native()
    {
        recorder.bind(native.0.clone(), "<id:session>".into());
    }
    let mut failure = process
        .send(&writer, Outbound::Message(codec.initialize()))
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
                    let Some(prompt) = codec.start(turn, text) else {
                        if !emit(out, thread, epoch, ProviderEvent::Session(
                            SessionEvent::StartTurnRefused { turn }
                        )).await { consumer_gone = true; break; }
                        continue;
                    };
                    #[cfg(debug_assertions)]
                    if let Some(recorder) = &mut process.recorder {
                        recorder.bind(turn.to_string(), format!("<id:turn:{turn}>"));
                    }
                    *active_turn.lock().unwrap() = codec.active().map(ActiveTurn::target);
                    if let Err(error) = process.send(&writer, prompt) {
                        failure = Some(error.to_string());
                        break;
                    }
                }
                Some(SessionControl::Shutdown) | None => {
                    let lines = codec.interrupt_active(StopReason::AppClosing);
                    for line in lines {
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
                Some(SessionControl::Interrupt { turn, reason }) => {
                    for line in codec.interrupt_lines(turn, reason) {
                        if let Err(error) = process.send(&writer, line) {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
                Some(SessionControl::InterruptActive(reason)) => {
                    for line in codec.interrupt_active(reason) {
                        if let Err(error) = process.send(&writer, line) {
                            failure = Some(error.to_string());
                            break;
                        }
                    }
                }
                Some(SessionControl::Answer { native, reply }) => {
                    if let Some(line) = codec.answer(native, reply)
                        && let Err(error) = process.send(&writer, Outbound::Message(line))
                    {
                        failure = Some(error.to_string());
                        break;
                    }
                }
            },
            result = &mut write_task => {
                writer_done = true;
                failure = Some(match result {
                    Ok(Err(error)) => format!("{} stdin writer failed: {error}", provider.name()),
                    Err(error) => format!("{} stdin writer task failed: {error}", provider.name()),
                    Ok(Ok(())) => format!("{} stdin writer closed", provider.name()),
                });
                break;
            }
            _ = async {
                match codec.next_deadline() {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                let timer_lines = match codec.timer_lines() { Ok(lines) => lines, Err(error) => { failure = Some(error.to_string()); break; } };
                for line in timer_lines {
                    if let Err(error) = process.send(&writer, line) { failure = Some(error.to_string()); break; }
                }
                if failure.is_some() { break; }
                if let Some(signal) = codec.escalate_interrupt() {
                    let kill = matches!(signal, Signal::Kill);
                    let _ = platform::signal_group(pid, signal);
                    if kill { break; }
                }
            }
            line = lines.next() => match line {
                Ok(Some(Line::Oversize(length))) => {
                    tracing::info!(length, provider = provider.name(), "skipped oversized provider line");
                    if !emit(out, thread, epoch, ProviderEvent::Session(SessionEvent::Notice {
                        text: format!("{} sent a line larger than 4 MiB; Veyra skipped it.", provider.name())
                    })).await { consumer_gone = true; break; }
                }
                Ok(Some(Line::Text(line))) => {
                    let wire = match serde_json::from_str::<serde_json::Value>(&line) {
                        Ok(wire) => wire,
                        Err(error) => {
                            tracing::info!(length = line.len(), "skipped non-JSON provider line");
                            if profile.reject_non_json { rejected = Some(format!("{} protocol: {error}", provider.name())); }
                            continue;
                        }
                    };
                    rejected = None;
                    match process.record_inbound(wire, &mut codec) {
                        Ok(mut decoded) => {
                            *active_turn.lock().unwrap() = codec.active().map(ActiveTurn::target);
                            for line in decoded.take_outbound() {
                                if let Err(error) = process.send(&writer, line) { failure = Some(error.to_string()); break; }
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
    codec.eof();
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
    let lines = tail
        .lock()
        .unwrap()
        .iter()
        .map(|line| {
            process
                .mcp
                .as_ref()
                .map_or_else(|| line.clone(), |mcp| mcp.redact(line, "[redacted]"))
        })
        .collect::<Vec<_>>();
    let failure = failure.map(|text| {
        process
            .mcp
            .as_ref()
            .map_or_else(|| text.clone(), |mcp| mcp.redact(&text, "[redacted]"))
    });
    let failure = failure.or(rejected).map(|text| with_stderr(&text, &lines));
    let missing = profile.resume_from_stderr
        && resume
        && lines
            .iter()
            .any(|line| line.contains("No conversation found with session ID"));
    let crash = if consumer_gone || missing {
        None
    } else if let Some(active) = codec.active() {
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
                    provider_crash_text(provider, observed.and_then(|status| status.code()), &lines)
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
        && codec.active().is_none()
        && (!ready || status.is_some_and(|status| !status.success()))
    {
        Some(provider_crash_text(
            provider,
            status.and_then(|status| status.code()),
            &lines,
        ))
    } else {
        crash
    };
    Completion::Exited(crash)
}

fn with_stderr(text: &str, lines: &[String]) -> String {
    let mut text = record::redact(text, None);
    for line in lines.iter().skip(lines.len().saturating_sub(8)) {
        text.push('\n');
        text.push_str(
            &record::redact(line, None)
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
                        tracing::warn!(%error, "could not wait for provider exit");
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
            record::Recorder::create(
                &path,
                ProviderKind::Claude,
                &version,
                cwd,
                env.home.as_deref(),
                None,
            )
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
fn start_task(
    process: Option<Process>,
    spec: SessionSpec,
    env: ChildEnv,
    out: mpsc::Sender<EngineMsg>,
    epoch: u32,
) -> SessionHandle {
    start_provider_task(ProviderKind::Claude, process, spec, env, out, epoch)
}
fn provider_crash_text(provider: ProviderKind, code: Option<i32>, lines: &[String]) -> String {
    with_stderr(
        &format!(
            "{} exited unexpectedly (exit code {})",
            provider.binary(),
            code.map(|code| code.to_string())
                .unwrap_or_else(|| "unknown".into())
        ),
        lines,
    )
}

struct SessionProfile {
    provider: ProviderKind,
    lines: fn(tokio::process::ChildStdout) -> SessionLines,
    reject_non_json: bool,
    resume_from_stderr: bool,
}
enum SessionLines {
    Direct(ByteLines<tokio::process::ChildStdout>),
    Rpc(jsonrpc::Reader),
}
impl SessionLines {
    async fn next(&mut self) -> std::io::Result<Option<Line>> {
        match self {
            Self::Direct(lines) => lines.next().await,
            Self::Rpc(lines) => Ok(lines.next().await?.map(Line::Text)),
        }
    }
}
enum Codec {
    Claude(Box<claude::Codec>),
    Codex(Box<codex::Codec>),
}
impl Codec {
    fn profile(&self) -> SessionProfile {
        match self {
            Self::Claude(_) => SessionProfile {
                provider: ProviderKind::Claude,
                lines: |stdout| SessionLines::Direct(ByteLines::new(stdout)),
                reject_non_json: true,
                resume_from_stderr: true,
            },
            Self::Codex(_) => SessionProfile {
                provider: ProviderKind::Codex,
                lines: |stdout| SessionLines::Rpc(jsonrpc::Reader::start(stdout)),
                reject_non_json: false,
                resume_from_stderr: false,
            },
        }
    }
    #[cfg(debug_assertions)]
    fn native(&self) -> Option<&NativeSessionId> {
        match self {
            Self::Claude(c) => Some(c.native()),
            Self::Codex(c) => c.native(),
        }
    }
    fn initialize(&mut self) -> serde_json::Value {
        match self {
            Self::Claude(_) => claude::initialize(),
            Self::Codex(c) => c.initialize(),
        }
    }
    fn active(&self) -> Option<ActiveTurn> {
        match self {
            Self::Claude(c) => c.active,
            Self::Codex(c) => c.active(),
        }
    }
    fn start(&mut self, turn: TurnId, text: String) -> Option<Outbound> {
        match self {
            Self::Claude(c) => c.start(turn, text),
            Self::Codex(c) => c.start(turn, text),
        }
    }
    fn value(&mut self, wire: serde_json::Value) -> Result<Decoded, ProviderError> {
        match self {
            Self::Claude(c) => c.value(wire),
            Self::Codex(c) => c.value(wire),
        }
    }
    fn answer(
        &mut self,
        native: NativeRequestRef,
        reply: super::Reply,
    ) -> Option<serde_json::Value> {
        match self {
            Self::Claude(c) => c.answer(native, reply),
            Self::Codex(c) => c.answer(native, reply),
        }
    }
    fn interrupt_active(&mut self, reason: StopReason) -> Vec<Outbound> {
        match self {
            Self::Claude(c) => c.interrupt_active(reason),
            Self::Codex(c) => c.interrupt_active(reason),
        }
    }
    fn interrupt_lines(&mut self, turn: TurnId, reason: StopReason) -> Vec<Outbound> {
        match self {
            Self::Claude(c) => c.interrupt_lines(turn, reason),
            Self::Codex(c) => c.interrupt_lines(turn, reason),
        }
    }
    fn next_deadline(&self) -> Option<tokio::time::Instant> {
        match self {
            Self::Claude(c) => c.next_deadline(),
            Self::Codex(c) => c.next_deadline(),
        }
    }
    fn escalate_interrupt(&mut self) -> Option<Signal> {
        match self {
            Self::Claude(c) => c.escalate_interrupt(),
            Self::Codex(c) => c.escalate_interrupt(),
        }
    }
    fn end_outcome(&self, outcome: TurnOutcome) -> TurnOutcome {
        match self {
            Self::Claude(c) => c.end_outcome(outcome),
            Self::Codex(c) => c.end_outcome(outcome),
        }
    }
    fn timer_lines(&mut self) -> Result<Vec<Outbound>, ProviderError> {
        match self {
            Self::Claude(_) => Ok(vec![]),
            Self::Codex(c) => c.timer_lines(),
        }
    }
    fn eof(&mut self) {
        if let Self::Codex(c) = self {
            c.eof()
        }
    }
}
pub async fn probe_codex(
    env: &ChildEnv,
    cwd: &std::path::Path,
    mut cancelled: oneshot::Receiver<()>,
) -> Result<super::Probe, ProviderError> {
    let binary = env.resolve(ProviderKind::Codex)?;
    #[cfg(debug_assertions)]
    let fixture = env
        .fixtures
        .as_ref()
        .map(|root| root.join("probe-codex.jsonl"));
    #[cfg(not(debug_assertions))]
    let fixture: Option<std::path::PathBuf> = None;
    check_cancelled(&mut cancelled)?;
    let mut child = env.spawn(&binary, &["app-server".into()], cwd, fixture.as_deref())?;
    let mut reader = jsonrpc::Reader::start(child.stdout().expect("piped stdout"));
    let stderr = discard_stderr(&mut child);
    let (writer, input) = mpsc::unbounded_channel();
    let write_task = tokio::spawn(write(
        child.stdin().expect("piped stdin"),
        input,
        Default::default(),
        env.closing.clone(),
        #[cfg(test)]
        Default::default(),
    ));
    let result = tokio::select! {
        _ = &mut cancelled => Err(ProviderError::Cancelled),
        result = tokio::time::timeout(Duration::from_secs(5), codex::probe_exchange(&mut reader, &writer, binary)) => {
            result.unwrap_or_else(|_|Err(ProviderError::Protocol("Codex probe timed out".into())))
        }
    };
    drop(writer);
    write_task.abort();
    let _ = write_task.await;
    child.stop().await?;
    stderr.abort();
    let _ = stderr.await;
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
        let process = Process::test(
            child,
            NativeSessionId("fixture-session".into()),
            None,
            #[cfg(debug_assertions)]
            None,
        );
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
            run_one(process, &mut control, &out, ThreadId::new(), 1, false).await
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
            let process = Process::test(
                child,
                NativeSessionId("fixture-session".into()),
                None,
                #[cfg(debug_assertions)]
                None,
            );
            let (sender, control) = mpsc::unbounded_channel();
            let mut control = SessionControls::new(control, Default::default());
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
        let process = Process::test(
            child,
            NativeSessionId("fixture-session".into()),
            None,
            #[cfg(debug_assertions)]
            None,
        );
        let (sender, control) = mpsc::unbounded_channel();
        let mut control = SessionControls::new(control, Default::default());
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
        let process = Process::test(
            child,
            NativeSessionId("fixture-session".into()),
            None,
            #[cfg(debug_assertions)]
            None,
        );
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
            run_one(process, &mut control, &out, ThreadId::new(), 1, false).await
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
        let process = Process::test(
            group,
            NativeSessionId("fixture-session".into()),
            None,
            #[cfg(debug_assertions)]
            None,
        );
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
                root: false,
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
        supervise(task, reaped, out, ThreadId::new(), 1, ProviderKind::Claude).await;
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
        let process = Process::test(
            child,
            NativeSessionId("fixture-session".into()),
            None,
            #[cfg(debug_assertions)]
            None,
        );
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
    fn provider_crash_text_bounds_and_redacts() {
        assert_eq!(
            provider_crash_text(ProviderKind::Claude, None, &[]),
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
        let lines: Vec<_> = lines
            .iter()
            .map(|line| record::redact(line, Some(token)))
            .collect();
        let text = provider_crash_text(ProviderKind::Claude, Some(7), &lines);
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
        let process = Process::test(
            group,
            NativeSessionId("fixture-session".into()),
            None,
            #[cfg(debug_assertions)]
            None,
        );
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
                root: false,
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
    #[cfg(debug_assertions)]
    async fn assert_stand_in_starts(
        env: &ChildEnv,
        cwd: &std::path::Path,
        marker: &std::path::Path,
    ) {
        let child = env
            .spawn(env.claude_bin.as_ref().unwrap(), &[], cwd, None)
            .unwrap();
        let process = child;
        let started = tokio::time::timeout(Duration::from_secs(10), async {
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

    #[cfg(debug_assertions)]
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
                let process = Process::test(
                    child,
                    NativeSessionId("missing-native".into()),
                    None,
                    #[cfg(debug_assertions)]
                    None,
                );
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
                        root: false,
                    },
                    env.clone(),
                    out,
                    1,
                );
                if during_cleanup {
                    tokio::time::timeout(Duration::from_secs(10), async {
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
                let exited = tokio::time::timeout(Duration::from_secs(10), async {
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

    #[cfg(debug_assertions)]
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
                    root: false,
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
            let exited = tokio::time::timeout(Duration::from_secs(10), async {
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
    #[cfg(debug_assertions)]
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
                    root: false,
                },
                env.clone(),
                out,
                1,
            )
            .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
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
                    tokio::time::timeout(Duration::from_secs(10), async {
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
            tokio::time::timeout(Duration::from_secs(10), async {
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
    #[cfg(debug_assertions)]
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
                    root: false,
                },
                env.clone(),
                out,
                1,
            )
            .unwrap();
            tokio::time::timeout(Duration::from_secs(10), async {
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
                    tokio::time::timeout(Duration::from_secs(10), async {
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
            tokio::time::timeout(Duration::from_secs(10), async {
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

#[cfg(all(test, debug_assertions))]
mod mcp_record_tests {
    use super::*;
    #[tokio::test]
    async fn codex_attempt_credentials_are_redacted_from_diagnostics() {
        let root = std::path::PathBuf::from("/tmp").join(format!("veyra-mcp-{}", ThreadId::new()));
        platform::private_dir(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let (out, _events) = mpsc::channel(16);
        let server = Arc::new(
            crate::mcp::Server::bind(&root, std::env::current_exe().unwrap(), out).unwrap(),
        );
        let thread = ThreadId::new();
        let config = Arc::new(codex::McpConfig::new(server, thread, 1, std::process::id()));
        let mut codec = codex::Codec::new(
            SessionSpec {
                thread,
                cwd: root,
                config: ThreadConfig {
                    mode: PermissionMode::AskFirst,
                    model: None,
                    effort: None,
                },
                native: Some(NativeSessionId("lost-native".into())),
                import: false,
                root: true,
            },
            std::process::id(),
            Some(config.clone()),
        );
        codec.initialize();
        let mut first = codec
            .value(serde_json::json!({"id":1,"result":{"userAgent":"fixture"}}))
            .unwrap();
        let first =
            String::from_utf8(first.take_outbound().pop().unwrap().into_bytes().unwrap()).unwrap();
        let mut next = codec.value(serde_json::json!({"id":2,"error":{"code":-32600,"message":"no rollout found for thread id lost-native"}})).unwrap();
        let next =
            String::from_utf8(next.take_outbound().pop().unwrap().into_bytes().unwrap()).unwrap();
        let mcp = RootMcp::Codex(config);
        for wire in [first, next] {
            let redacted = mcp.redact(&wire, "[redacted]");
            assert!(
                redacted.contains("\"VEYRA_MCP_TOKEN\":\"[redacted]\""),
                "attempt credential reached diagnostics"
            );
            let diagnostic = with_stderr(&format!("Provider failed: {redacted}"), &[redacted]);
            assert!(diagnostic.contains("[redacted]"));
        }
    }
    #[tokio::test]
    async fn session_recorder_replaces_token_before_writing() {
        let root = std::path::PathBuf::from("/tmp").join(format!(
            "veyra-mcp-rec-{}",
            &ThreadId::new().to_string()[28..]
        ));
        platform::private_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let (out, _events) = mpsc::channel(16);
        let server = Arc::new(
            crate::mcp::Server::bind(&root, std::env::current_exe().unwrap(), out).unwrap(),
        );
        let token = crate::mcp::McpToken::new();
        let config = crate::mcp::Configuration::create(
            &root,
            ThreadId::new(),
            &server.command,
            &server.socket,
            &token,
        )
        .unwrap();
        let config_path = config.path.clone();
        let capture = root.join("capture.jsonl");
        let child = ChildEnv::load()
            .spawn(
                std::path::Path::new("/bin/sh"),
                &["-c".into(), "read ignored".into()],
                &root,
                None,
            )
            .unwrap();
        let mut process = Process::test(
            child,
            NativeSessionId("record-only".into()),
            Some(crate::mcp::Session {
                config,
                hash: crate::mcp::TokenHash(token.hash()),
                server,
            }),
            #[cfg(debug_assertions)]
            Some(
                record::Recorder::create(&capture, ProviderKind::Claude, "fake", &root, None, None)
                    .unwrap(),
            ),
        );
        let wire = serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":format!("prefix {} suffix",token.expose())}]}});
        let decoded = process.io.record_inbound(wire, &mut process.codec).unwrap();
        let texts: Vec<_> = decoded
            .events
            .iter()
            .filter_map(|event| match event {
                ProviderEvent::Turn {
                    event: TurnEvent::TextDelta { text, .. },
                    ..
                } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            texts == ["prefix [redacted] suffix"],
            "credential reached provider decoding"
        );
        let recorded = std::fs::read_to_string(&capture).unwrap();
        assert!(
            !recorded.contains(token.expose()),
            "credential reached the capture file"
        );
        assert!(recorded.contains("<id:token>"));
        let Process {
            io:
                SessionIo {
                    child,
                    mcp,
                    recorder,
                },
            ..
        } = process;
        child.stop().await.unwrap();
        drop(recorder);
        drop(mcp);
        assert!(!config_path.exists());
    }
}

#[cfg(all(test, debug_assertions))]
mod recorder_tests {
    use super::*;
    #[tokio::test]
    async fn unrecognized_reply_cannot_replace_recorded_thread_metadata() {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/provider-metadata-{}",
            ThreadId::new()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let env = ChildEnv::isolated();
        let child = env
            .spawn(
                std::path::Path::new("/bin/sleep"),
                &["5".into()],
                &root,
                None,
            )
            .unwrap();
        let path = root.join("capture.jsonl");
        let mut process = Process {
            codec: Codec::Codex(Box::new(codex::Codec::new(
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: root.clone(),
                    config: ThreadConfig {
                        mode: PermissionMode::AskFirst,
                        model: None,
                        effort: None,
                    },
                    native: None,
                    import: false,
                    root: false,
                },
                child.pid(),
                None,
            ))),
            io: SessionIo {
                child,
                mcp: None,
                recorder: Some(
                    record::Recorder::create(
                        &path,
                        ProviderKind::Codex,
                        "unknown",
                        &root,
                        None,
                        None,
                    )
                    .unwrap(),
                ),
            },
        };
        process.codec.initialize();
        process
            .codec
            .value(serde_json::json!({"id":1,"result":{"userAgent":"fixture"}}))
            .unwrap();
        let known = serde_json::json!({"id":2,"result":{"thread":{"id":"known-native","cliVersion":"known-version"}}});
        process
            .io
            .record_inbound(known, &mut process.codec)
            .unwrap();
        let unrelated = serde_json::json!({"id":999,"result":{"thread":{"id":"unrelated-native","cliVersion":"unrelated-version"}}});
        process
            .io
            .record_inbound(unrelated, &mut process.codec)
            .unwrap();
        process.io.child.stop().await.unwrap();
        drop(process.io.recorder);
        let capture: Vec<serde_json::Value> = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(capture[0]["cli_version"], "known-version");
        assert_eq!(capture[1]["line"]["result"]["thread"]["id"], "<id:session>");
        assert_eq!(
            capture[2]["line"]["result"]["thread"]["id"],
            "unrelated-native"
        );
        assert!(env.groups.lock().unwrap().is_empty());
    }
}

#[cfg(test)]
mod availability_tests {
    use super::*;
    #[tokio::test]
    async fn spawning_uses_provider_availability() {
        for provider in ProviderKind::ALL {
            let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../target/availability-{}", ThreadId::new()));
            std::fs::create_dir_all(&root).unwrap();
            let _scratch = crate::tests::Scratch(root.clone());
            let binary = root.join("owned-provider-stand-in");
            let marker = root.join("spawned");
            std::fs::write(
                &binary,
                "#!/bin/sh\nprintf launched > spawned\nwhile read ignored; do :; done\n",
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
            let env = ChildEnv::isolated().with_binary(provider, binary);
            let (out, mut events) = mpsc::channel(16);
            let result = spawn_provider(
                provider,
                SessionSpec {
                    thread: ThreadId::new(),
                    cwd: root,
                    config: ThreadConfig {
                        mode: PermissionMode::AskFirst,
                        model: None,
                        effort: None,
                    },
                    native: None,
                    import: false,
                    root: false,
                },
                env.clone(),
                out,
                1,
            );
            if !provider.is_built() {
                assert!(matches!(result, Err(ProviderError::NotBuilt)));
                assert!(!marker.exists());
                continue;
            }
            let handle = result.unwrap();
            let end = tokio::time::Instant::now() + Duration::from_secs(3);
            while !marker.exists() {
                assert!(
                    tokio::time::Instant::now() < end,
                    "built provider never spawned"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            handle.control.send(SessionControl::Shutdown).unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while let Some(event) = events.recv().await {
                    if matches!(
                        event,
                        EngineMsg::Provider {
                            event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                            ..
                        }
                    ) {
                        return;
                    }
                }
                panic!("session closed without an exit event")
            })
            .await
            .unwrap();
            assert!(env.groups.lock().unwrap().is_empty());
        }
    }
}

#[cfg(test)]
mod git_metadata_tests {
    use super::*;

    #[tokio::test]
    async fn metadata_roots_are_bounded_and_fail_closed() {
        let root = std::path::PathBuf::from("/tmp")
            .join(format!("veyra-git-metadata-{}", ThreadId::new()));
        std::fs::create_dir(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let binaries = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../target/git-metadata-{}", ThreadId::new()));
        std::fs::create_dir_all(&binaries).unwrap();
        let _binaries = crate::tests::Scratch(binaries.clone());
        let git = binaries.join("git");
        let (cleanup, mut pending) = mpsc::unbounded_channel();
        let mut env = ChildEnv::isolated();
        env.path = binaries.into_os_string();
        assert!(codex_git_roots(&env, &root, &cleanup).await.is_empty());
        std::fs::create_dir(root.join("metadata")).unwrap();
        std::fs::create_dir(root.join("common")).unwrap();
        for (body, expected) in [
            ("printf 'metadata\\nmetadata\\n'", 0),
            ("printf 'metadata\\ncommon\\n'", 0),
            ("printf 'metadata\\n'", 0),
            ("printf 'missing\\nmissing\\n'", 0),
            ("exit 1", 0),
            ("/bin/sleep 10", 0),
            ("/usr/bin/head -c 8193 /dev/zero", 0),
        ] {
            std::fs::write(&git, format!("#!/bin/sh\n{body}\n")).unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&git)
                    .status()
                    .unwrap()
                    .success()
            );
            let start = std::time::Instant::now();
            let roots = codex_git_roots(&env, &root, &cleanup).await;
            assert_eq!(roots.len(), expected, "{body}");
            assert!(start.elapsed() < Duration::from_secs(3));
            if let Some(path) = roots.first() {
                assert_eq!(*path, root.join("metadata").canonicalize().unwrap());
            }
            while let Ok(done) = pending.try_recv() {
                let _ = done.await;
            }
            assert!(env.groups.lock().unwrap().is_empty());
        }
    }
}

#[cfg(test)]
mod git_security_tests {
    use super::*;
    use crate::tests::{CapturedLogs, Scratch};
    use std::path::PathBuf;

    struct GitFixture {
        _scratch: Scratch,
        _binaries: Scratch,
        env: ChildEnv,
        project: PathBuf,
        common: PathBuf,
        dir: PathBuf,
    }
    impl GitFixture {
        fn new(kind: &str) -> Self {
            let root =
                PathBuf::from("/tmp").join(format!("veyra-git-security-{}", ThreadId::new()));
            std::fs::create_dir(&root).unwrap();
            let scratch = Scratch(root.clone());
            let root = root.canonicalize().unwrap();
            let worktree = root.join("worktree");
            let project = worktree.join("nested");
            std::fs::create_dir_all(&project).unwrap();
            let common = root.join("repository/.git");
            let dir = common.join("worktrees/current");
            for path in [
                &dir,
                &common.join("objects"),
                &common.join("refs"),
                &common.join("logs"),
                &common.join("hooks"),
            ] {
                std::fs::create_dir_all(path).unwrap();
            }
            std::fs::write(common.join("config"), "fixture config").unwrap();
            std::fs::write(dir.join("commondir"), "../..\n").unwrap();
            std::fs::write(
                dir.join("gitdir"),
                format!("{}\n", worktree.join(".git").display()),
            )
            .unwrap();
            std::fs::write(
                worktree.join(".git"),
                format!("gitdir: {}\n", dir.display()),
            )
            .unwrap();
            let (reported_dir, reported_common) = match kind {
                "head_objects_refs" => {
                    let foreign = root.join("elsewhere/meta");
                    std::fs::create_dir_all(foreign.join("objects")).unwrap();
                    std::fs::create_dir_all(foreign.join("refs")).unwrap();
                    std::fs::write(foreign.join("HEAD"), "ref: refs/heads/main\n").unwrap();
                    std::fs::write(
                        worktree.join(".git"),
                        format!("gitdir: {}\n", foreign.display()),
                    )
                    .unwrap();
                    #[cfg(debug_assertions)]
                    {
                        let output = std::process::Command::new("git")
                            .args(["rev-parse", "--git-dir", "--git-common-dir"])
                            .current_dir(&project)
                            .output()
                            .unwrap();
                        assert!(output.status.success());
                        let printed: Vec<_> = std::str::from_utf8(&output.stdout)
                            .unwrap()
                            .lines()
                            .map(|path| project.join(path).canonicalize().unwrap())
                            .collect();
                        assert_eq!(printed, [foreign.clone(), foreign.clone()]);
                    }
                    let mut names: Vec<_> = std::fs::read_dir(&foreign)
                        .unwrap()
                        .map(|entry| entry.unwrap().file_name())
                        .collect();
                    names.sort();
                    assert_eq!(names, ["HEAD", "objects", "refs"]);
                    (foreign.clone(), foreign)
                }
                "foreign_repository" => {
                    let foreign = root.join("foreign/.git");
                    std::fs::create_dir_all(&foreign).unwrap();
                    std::fs::write(
                        worktree.join(".git"),
                        format!("gitdir: {}\n", foreign.display()),
                    )
                    .unwrap();
                    (foreign.clone(), foreign)
                }
                "foreign_common" => {
                    let foreign = root.join("foreign/.git");
                    std::fs::create_dir_all(&foreign).unwrap();
                    std::fs::write(dir.join("commondir"), format!("{}\n", foreign.display()))
                        .unwrap();
                    (dir.clone(), foreign)
                }
                "foreign_backlink" => {
                    let other = root.join("other-worktree");
                    std::fs::create_dir(&other).unwrap();
                    std::fs::write(other.join(".git"), format!("gitdir: {}\n", dir.display()))
                        .unwrap();
                    std::fs::write(
                        dir.join("gitdir"),
                        format!("{}\n", other.join(".git").display()),
                    )
                    .unwrap();
                    (dir.clone(), common.clone())
                }
                "plain" => {
                    std::fs::remove_file(worktree.join(".git")).unwrap();
                    std::fs::create_dir(worktree.join(".git")).unwrap();
                    (worktree.join(".git"), worktree.join(".git"))
                }
                "linked" => (dir.clone(), common.clone()),
                _ => panic!("unknown security fixture"),
            };
            let binaries = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../target/git-security-{}", ThreadId::new()));
            std::fs::create_dir_all(&binaries).unwrap();
            let binary_scratch = Scratch(binaries.clone());
            let git = binaries.join("git");
            std::fs::write(
                &git,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' '{}' '{}'\n",
                    reported_dir.display(),
                    reported_common.display()
                ),
            )
            .unwrap();
            assert!(
                std::process::Command::new("/bin/chmod")
                    .arg("700")
                    .arg(&git)
                    .status()
                    .unwrap()
                    .success()
            );
            let mut env = ChildEnv::isolated();
            env.path = binaries.clone().into_os_string();
            Self {
                _scratch: scratch,
                _binaries: binary_scratch,
                env,
                project,
                common,
                dir,
            }
        }
        async fn roots(&self) -> (Vec<PathBuf>, usize) {
            let logs = CapturedLogs::default();
            let _capture = tracing::subscriber::set_default(logs.clone());
            let (cleanup, mut pending) = mpsc::unbounded_channel();
            let roots = codex_git_roots(&self.env, &self.project, &cleanup).await;
            while let Ok(done) = pending.try_recv() {
                let _ = done.await;
            }
            assert!(self.env.groups.lock().unwrap().is_empty());
            let lines = logs
                .0
                .lock()
                .unwrap()
                .iter()
                .filter(|row| {
                    row.get("message")
                        .is_some_and(|text| text.contains("no writable roots added"))
                })
                .count();
            (roots, lines)
        }
    }
    async fn refuses(kind: &str) {
        let fixture = GitFixture::new(kind);
        let (roots, lines) = fixture.roots().await;
        assert!(roots.is_empty(), "{kind} escaped the sandbox: {roots:?}");
        assert_eq!(lines, 1, "{kind} must log exactly one refusal");
    }
    #[tokio::test]
    async fn codex_git_rejects_exact_review_head_objects_refs_probe() {
        refuses("head_objects_refs").await;
    }
    #[tokio::test]
    async fn codex_git_rejects_foreign_repository_pointer() {
        refuses("foreign_repository").await;
    }
    #[tokio::test]
    async fn codex_git_rejects_foreign_common_directory() {
        refuses("foreign_common").await;
    }
    #[tokio::test]
    async fn codex_git_rejects_foreign_worktree_backlink() {
        refuses("foreign_backlink").await;
    }
    #[tokio::test]
    async fn codex_git_plain_repository_adds_nothing() {
        refuses("plain").await;
    }
    #[tokio::test]
    async fn codex_git_rejects_symlinked_common_storage() {
        let fixture = GitFixture::new("linked");
        let objects = fixture.common.join("objects");
        std::fs::remove_dir(&objects).unwrap();
        platform::test_symlink(&fixture.common.join("hooks"), &objects).unwrap();
        let (roots, lines) = fixture.roots().await;
        assert!(roots.is_empty());
        assert_eq!(lines, 1);
    }
    #[tokio::test]
    async fn codex_git_linked_worktree_uses_only_narrow_metadata_roots() {
        let fixture = GitFixture::new("linked");
        let (roots, lines) = fixture.roots().await;
        assert_eq!(
            roots,
            [
                fixture.dir.clone(),
                fixture.common.join("objects"),
                fixture.common.join("refs"),
                fixture.common.join("logs")
            ]
        );
        assert_eq!(lines, 0);
        assert!(!roots.iter().any(|root| root == &fixture.common
            || root == &fixture.common.join("hooks")
            || root == &fixture.common.join("config")));
        assert_ne!(fixture.dir, fixture.common);
    }
}
