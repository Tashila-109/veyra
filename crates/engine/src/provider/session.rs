use super::{
    ProviderError, ProviderEvent, ProviderKind, SessionControl, SessionEvent, SessionHandle,
    SessionSpec, TurnEvent,
    claude::{self, Codec},
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
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Child,
    sync::mpsc,
};

struct ProcessGroup {
    child: Option<Child>,
    pid: u32,
}
fn process_alive(child: &mut Child, pid: u32) -> std::io::Result<bool> {
    child.try_wait()?;
    platform::group_exists(pid)
}
async fn end_process(
    mut child: Child,
    pid: u32,
    check: fn(&mut Child, u32) -> std::io::Result<bool>,
) -> std::io::Result<std::process::ExitStatus> {
    let _ = platform::signal_group(pid, Signal::Terminate);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    let mut failure = None;
    loop {
        match check(&mut child, pid) {
            Err(error) => {
                failure = Some(error);
                let _ = platform::signal_group(pid, Signal::Kill);
                break;
            }
            Ok(false) => break,
            Ok(true) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = platform::signal_group(pid, Signal::Kill);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status = child.wait().await?;
    failure.map_or(Ok(status), Err)
}
impl ProcessGroup {
    fn new(child: Child) -> Self {
        Self {
            pid: child.id().expect("new child"),
            child: Some(child),
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
    async fn stop(mut self) -> std::io::Result<std::process::ExitStatus> {
        match self.child.take() {
            Some(child) => tokio::spawn(end_process(child, self.pid, process_alive))
                .await
                .map_err(std::io::Error::other)?,
            None => Err(std::io::Error::other("process already stopped")),
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            tokio::spawn(end_process(child, self.pid, process_alive));
        }
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
    fn record_inbound(&mut self, line: &str) -> Result<(), ProviderError> {
        #[cfg(debug_assertions)]
        if let Some(recorder) = &mut self.recorder {
            recorder.line(false, serde_json::from_str(line)?)?;
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
        child: ProcessGroup::new(child),
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
    let process = launch(&spec, &env, None)?;
    let (control, receiver) = mpsc::unbounded_channel();
    let thread = spec.thread;
    let supervisor_out = out.clone();
    let task = tokio::spawn(async move {
        let mut spec = spec;
        let mut process = process;
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
                        Ok(next) => {
                            let notice = ProviderEvent::Session(SessionEvent::Notice {
                                text: "The earlier conversation could not be resumed; this is a new session".into(),
                            });
                            if !emit(&out, thread, epoch, notice).await {
                                return false;
                            }
                            process = next;
                        }
                        Err(error) => {
                            return emit(
                                &out,
                                thread,
                                epoch,
                                ProviderEvent::Session(SessionEvent::Exited {
                                    crash: Some(record::redact(&error.to_string(), None)),
                                }),
                            )
                            .await;
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
    tokio::spawn(async move {
        if let Err(error) = task.await {
            let _ = emit(
                &supervisor_out,
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
    });
    Ok(SessionHandle { control })
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
    let mut lines = BufReader::new(process.child.stdout().expect("piped stdout")).lines();
    let mut errors = BufReader::new(process.child.stderr().expect("piped stderr")).lines();
    let tail = Arc::new(Mutex::new(VecDeque::<String>::new()));
    let stderr_tail = tail.clone();
    let mut stderr = tokio::spawn(async move {
        while let Ok(Some(line)) = errors.next_line().await {
            let line = record::redact(&line, None).chars().take(500).collect();
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
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut process.recorder {
        recorder.bind(process.native.0.clone(), "<id:session>".into());
    }
    if let Err(error) = process.send(&writer, claude::initialize()) {
        return Completion::Exited(Some(error.to_string()));
    }
    let mut failure = None;
    let mut consumer_gone = false;
    let mut writer_done = false;
    loop {
        tokio::select! {
            _ = out.closed() => { consumer_gone = true; break; }
            result = &mut write_task => {
                writer_done = true;
                failure = Some(match result {
                    Ok(Err(error)) => format!("Claude stdin writer failed: {error}"),
                    Err(error) => format!("Claude stdin writer task failed: {error}"),
                    Ok(Ok(())) => "Claude stdin writer closed".into(),
                });
                break;
            }
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
                Some(SessionControl::Shutdown) | None => break,
                Some(SessionControl::Interrupt) => {
                    let line = serde_json::json!({
                        "type":"control_request", "request_id":"veyra-interrupt",
                        "request":{"subtype":"interrupt"}
                    });
                    if let Err(error) = process.send(&writer, line) {
                        failure = Some(error.to_string());
                        break;
                    }
                }
                Some(SessionControl::Answer { .. }) => {}
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    if let Err(error) = process.record_inbound(&line) {
                        failure = Some(error.to_string());
                        break;
                    }
                    match codec.line(&line) {
                        Ok(decoded) => {
                            for reply in decoded.replies {
                                if let Err(error) = process.send(&writer, reply) {
                                    failure = Some(error.to_string());
                                    break;
                                }
                            }
                            if failure.is_some() { break; }
                            for event in decoded.events {
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
        let text = failure
            .unwrap_or_else(|| crash_text(status.and_then(|status| status.code()), &lines, None));
        let _ = emit(
            out,
            thread,
            epoch,
            ProviderEvent::Turn {
                turn: active.id(),
                event: TurnEvent::TurnEnded {
                    outcome: TurnOutcome::Failed {
                        message: record::redact(&text, None),
                    },
                },
            },
        )
        .await;
        Some(text)
    } else if let Some(failure) = failure {
        Some(record::redact(&failure, None))
    } else if status.is_some_and(|status| !status.success()) {
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
    let mut text = format!(
        "claude exited unexpectedly (exit code {})",
        code.map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".into())
    );
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
            ProcessGroup::new(env.command(&binary, &["--version".into()], cwd).spawn()?);
        let mut stdout = BufReader::new(child.stdout().expect("piped version stdout")).lines();
        let result = tokio::select! {
            _ = &mut cancelled => Err(ProviderError::Cancelled),
            result = async {
                let line = stdout.next_line().await?.unwrap_or_default();
                Ok::<_, ProviderError>(line)
            } => result,
        };
        child.stop().await?;
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
    let mut child =
        ProcessGroup::new(env.spawn(&binary, &claude::args(None), cwd, fixture.as_deref())?);
    let mut lines = BufReader::new(child.stdout().expect("piped stdout")).lines();
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
            while let Some(line) = lines.next_line().await? {
                #[cfg(debug_assertions)]
                if let Some(recorder) = &mut recorder {
                    recorder.line(false, serde_json::from_str(&line)?)?;
                }
                if let Some(reply) = claude::probe_reply(&line)? {
                    return Ok(super::Probe {
                        provider: ProviderKind::Claude,
                        path: Some(binary.clone()),
                        version: (!version.is_empty()).then_some(version),
                        readiness: super::Readiness::Ready,
                        models: reply.models,
                        account: reply.account,
                    });
                }
            }
            Err(ProviderError::Protocol("Claude initialize ended without a reply".into()))
        } => result,
    };
    drop(stdin);
    let status = child.stop().await?;
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
    use super::*;
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
