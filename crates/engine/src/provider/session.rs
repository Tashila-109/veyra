#[cfg(debug_assertions)]
use super::env;
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

struct ProcessGroup(u32);
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        let _ = platform::signal_group(self.0, Signal::Kill);
    }
}
struct Process {
    child: Child,
    native: NativeSessionId,
    #[cfg(debug_assertions)]
    recorder: Option<record::Recorder>,
}
fn launch(
    spec: &SessionSpec,
    env: &ChildEnv,
    fresh: Option<NativeSessionId>,
) -> Result<Process, ProviderError> {
    let binary = env.resolve(ProviderKind::Claude)?;
    #[cfg(debug_assertions)]
    let (n, epoch) = env::next_lane(spec.thread);
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
                            if !emit(&out, thread, epoch, ProviderEvent::Session(SessionEvent::Notice { text: "The earlier conversation could not be resumed; this is a new session".into() })).await { return false; }
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
    let pid = process.child.id().expect("newly spawned child");
    let _group = ProcessGroup(pid);
    let mut lines = BufReader::new(process.child.stdout.take().expect("piped stdout")).lines();
    let mut errors = BufReader::new(process.child.stderr.take().expect("piped stderr")).lines();
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
    let mut stdin = process.child.stdin.take().expect("piped stdin");
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
    let initialize = claude::initialize();
    #[cfg(debug_assertions)]
    if let Some(recorder) = &mut process.recorder {
        recorder.bind(process.native.0, "<id:session>".into());
        if let Err(error) = recorder.line(true, initialize.clone()) {
            return Completion::Exited(Some(error.to_string()));
        }
    }
    let _ = writer.send(initialize);
    let mut failure = None;
    let mut consumer_gone = false;
    let mut writer_done = false;
    loop {
        tokio::select! {
            _ = out.closed() => { consumer_gone = true; break; }
            result = &mut write_task => {
                writer_done = true;
                failure = Some(match result { Ok(Err(error)) => format!("Claude stdin writer failed: {error}"),Err(error) => format!("Claude stdin writer task failed: {error}"),Ok(Ok(())) => "Claude stdin writer closed".into() });
                break;
            }
            message = control.recv() => match message {
                Some(SessionControl::StartTurn { turn, text }) => {
                    let line = codec.start(turn, text);
                    #[cfg(debug_assertions)]
                    if let Some(recorder) = &mut process.recorder {
                        recorder.bind(turn.to_string(), format!("<id:turn:{turn}>"));
                        if let Err(error) = recorder.line(true, line.clone()) { failure = Some(error.to_string()); break; }
                    }
                    if writer.send(line).is_err() { failure = Some("Claude stdin writer closed".into()); break; }
                }
                Some(SessionControl::Shutdown) | None => break,
                Some(SessionControl::Interrupt) => {
                    let line = serde_json::json!({"type":"control_request","request_id":"veyra-interrupt","request":{"subtype":"interrupt"}});
                    #[cfg(debug_assertions)]
                    if let Some(recorder) = &mut process.recorder && let Err(error) = recorder.line(true,line.clone()) { failure = Some(error.to_string()); break; }
                    let _ = writer.send(line);
                }
                Some(SessionControl::Answer { .. }) => {}
            },
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    #[cfg(debug_assertions)]
                    if let Some(recorder) = &mut process.recorder {
                        match serde_json::from_str(&line).map_err(std::io::Error::from).and_then(|value| recorder.line(false, value)) {
                            Ok(()) => {}, Err(error) => { failure = Some(error.to_string()); break; }
                        }
                    }
                    match codec.line(&line) {
                        Ok(decoded) => {
                            for reply in decoded.replies {
                                #[cfg(debug_assertions)]
                                if let Some(recorder) = &mut process.recorder && let Err(error) = recorder.line(true,reply.clone()) { failure = Some(error.to_string()); break; }
                                let _ = writer.send(reply);
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
    let status = process.child.try_wait().ok().flatten();
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
    let crash = if let Some(turn) = codec.current {
        let text = failure
            .unwrap_or_else(|| crash_text(status.and_then(|status| status.code()), &lines, None));
        let _ = emit(
            out,
            thread,
            epoch,
            ProviderEvent::Turn {
                turn: Some(turn),
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
    let _ = process.child.start_kill();
    let _ = process.child.wait().await;
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

pub async fn probe(env: &ChildEnv) -> Result<super::Probe, ProviderError> {
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
        let mut child = env
            .command(&binary, &["--version".into()], &std::env::current_dir()?)
            .spawn()?;
        let _group = ProcessGroup(child.id().expect("newly spawned version probe"));
        let mut stdout = BufReader::new(child.stdout.take().expect("piped version stdout")).lines();
        let line = stdout.next_line().await?.unwrap_or_default();
        let _ = child.wait().await;
        line
    };
    let mut child = env.spawn(
        &binary,
        &claude::args(None),
        &std::env::current_dir()?,
        fixture.as_deref(),
    )?;
    let _group = ProcessGroup(child.id().expect("newly spawned probe"));
    let mut lines = BufReader::new(child.stdout.take().expect("piped stdout")).lines();
    let mut stdin = child.stdin.take().expect("piped stdin");
    stdin
        .write_all(format!("{}\n", claude::initialize()).as_bytes())
        .await?;
    stdin.flush().await?;
    let result = loop {
        match lines.next_line().await? {
            Some(line) => {
                if let Some(reply) = claude::probe_reply(&line)? {
                    break Ok(super::Probe {
                        provider: ProviderKind::Claude,
                        path: Some(binary.clone()),
                        version: Some(version),
                        readiness: super::Readiness::Ready,
                        models: reply.models,
                        account: reply.account,
                    });
                }
            }
            None => {
                break Err(ProviderError::Protocol(
                    "Claude initialize ended without a reply".into(),
                ));
            }
        }
    };
    drop(stdin);
    let _ = child.start_kill();
    let _ = child.wait().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
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
