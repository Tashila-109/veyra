pub mod claude;
pub mod codex;
pub mod env;
pub mod grok;
pub mod jsonrpc;
pub(crate) mod lines;
#[cfg(test)]
pub mod scripted;
pub mod session;
use crate::{actor::EngineMsg, domain::*};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Claude,
    Codex,
    Grok,
}
#[derive(Clone, Debug)]
pub struct SessionSpec {
    pub thread: ThreadId,
    pub cwd: PathBuf,
    pub config: ThreadConfig,
    pub native: Option<NativeSessionId>,
    pub import: bool,
    pub root: bool,
}
#[derive(Clone, Debug)]
pub struct SessionHandle {
    pub control: mpsc::UnboundedSender<SessionControl>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionControl {
    StartTurn {
        turn: TurnId,
        text: String,
    },
    Interrupt {
        turn: TurnId,
        reason: StopReason,
    },
    InterruptActive(StopReason),
    Answer {
        native: NativeRequestRef,
        reply: Reply,
    },
    Shutdown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopReason {
    User,
    QuestionChoice,
    Parent,
    StorageBlocked,
    AppClosing,
}
impl StopReason {
    pub fn text(self) -> &'static str {
        match self {
            Self::User => "Stopped by you",
            Self::QuestionChoice => "Stopped to send your answer",
            Self::Parent => "Stopped by its parent",
            Self::StorageBlocked => "Stopped because the database is blocked",
            Self::AppClosing => "Veyra closed while this turn ran",
        }
    }
}
pub type Reply = Decision;
#[derive(Clone, Debug)]
pub enum ProviderEvent {
    Session(SessionEvent),
    Turn {
        turn: Option<TurnId>,
        event: TurnEvent,
    },
}
#[derive(Clone, Debug)]
pub enum SessionEvent {
    StartTurnRefused {
        turn: TurnId,
    },
    ProviderTurnStarted {
        provider_turn: TurnId,
    },
    Ready {
        native: NativeSessionId,
        cli_version: Option<String>,
        pid: u32,
        pgid: u32,
        capabilities: Vec<String>,
    },
    Notice {
        text: String,
    },
    BackgroundWork {
        pending: bool,
    },
    Exited {
        crash: Option<String>,
    },
}
#[derive(Clone, Debug)]
pub enum TurnEvent {
    Started,
    Accepted,
    TextDelta {
        key: NativeItemKey,
        text: String,
        streamed: bool,
    },
    AsyncQuestions {
        key: NativeItemKey,
        text: String,
        questions: Vec<AsyncQuestion>,
    },
    ReasoningDelta {
        key: NativeItemKey,
        text: String,
    },
    ItemEnded {
        key: NativeItemKey,
    },
    ToolStarted {
        key: NativeItemKey,
        call: ToolCall,
    },
    ToolTitleChanged {
        key: NativeItemKey,
        title: String,
    },
    ToolFinished {
        key: NativeItemKey,
        ok: bool,
        output_tail: Option<String>,
    },
    ToolOutput {
        key: NativeItemKey,
        text: String,
    },
    Plan(Vec<PlanStep>),
    RequestOpened {
        native: NativeRequestRef,
        kind: RequestKind,
    },
    TurnEnded {
        outcome: TurnOutcome,
        park: bool,
    },
}
impl TurnEvent {
    pub fn opens_provider_turn(&self) -> bool {
        match self {
            Self::Started
            | Self::TextDelta { .. }
            | Self::AsyncQuestions { .. }
            | Self::ReasoningDelta { .. }
            | Self::ToolStarted { .. }
            | Self::Plan(_)
            | Self::RequestOpened { .. } => true,
            Self::Accepted
            | Self::ItemEnded { .. }
            | Self::ToolTitleChanged { .. }
            | Self::ToolFinished { .. }
            | Self::ToolOutput { .. }
            | Self::TurnEnded { .. } => false,
        }
    }
}
#[derive(Clone, Debug)]
pub struct TerminalHandle {
    pub control: mpsc::UnboundedSender<TerminalControl>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalControl {
    Stop,
}
#[derive(Clone, Debug)]
pub enum TerminalEvent {
    Exited,
}
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("provider work cancelled")]
    Cancelled,
    #[error("this provider is not built yet")]
    NotBuilt,
    #[error("{0} is not installed")]
    NotInstalled(String),
    #[error("fixture is missing: {0}")]
    FixtureMissing(PathBuf),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("Provider protocol: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Protocol(String),
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Readiness {
    Ready,
    NotInstalled,
    SignedOut,
    Checking,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Model {
    pub value: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "supportedEffortLevels")]
    pub effort_levels: Vec<String>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    pub provider: ProviderKind,
    pub path: Option<PathBuf>,
    pub version: Option<String>,
    pub readiness: Readiness,
    pub models: Vec<Model>,
    pub account: Option<serde_json::Value>,
}
pub(crate) fn signed_out_account(account: Option<&serde_json::Value>) -> bool {
    account
        .and_then(serde_json::Value::as_object)
        .is_some_and(|account| {
            account.contains_key("apiProvider")
                && account.contains_key("tokenSource")
                && !["email", "organization", "subscriptionType"]
                    .iter()
                    .any(|key| account.contains_key(*key))
        })
}
impl Probe {
    pub fn account_presence(&self) -> &'static str {
        if self.account.as_ref().is_some_and(|account| match account {
            serde_json::Value::Null => false,
            serde_json::Value::Object(object) => !object.is_empty(),
            _ => true,
        }) {
            "known"
        } else {
            "unknown"
        }
    }
}
impl ProviderKind {
    pub const ALL: [Self; 3] = [Self::Claude, Self::Codex, Self::Grok];
    pub fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Grok => "grok",
        }
    }
    pub fn install_hint(self) -> &'static str {
        match self {
            Self::Claude => "Run npm install -g @anthropic-ai/claude-code and check again.",
            Self::Codex => "Install the Codex CLI and check again.",
            Self::Grok => "Install the Grok CLI and check again.",
        }
    }
    pub fn sign_in_command(self) -> &'static str {
        match self {
            Self::Claude => "claude auth login",
            Self::Codex => "codex login",
            Self::Grok => "grok login",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
            Self::Grok => "Grok",
        }
    }
    async fn probe_cancellable(
        self,
        env: &env::ChildEnv,
        cwd: &std::path::Path,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) -> Probe {
        let result = match self {
            Self::Claude => Some(session::probe(env, cwd, cancelled).await),
            Self::Codex => Some(session::probe_codex(env, cwd, cancelled).await),
            Self::Grok => Some(session::probe_grok(env, cwd, cancelled).await),
        };
        let error = match result {
            Some(Ok(probe)) => return probe,
            Some(Err(error)) => Some(error),
            None => None,
        };
        if let Some(error) = &error {
            tracing::warn!(error = %crate::record::redact(&error.to_string(),None), "provider probe failed");
        }
        Probe {
            provider: self,
            path: env.resolve(self).ok(),
            version: None,
            readiness: if matches!(error, Some(ProviderError::NotInstalled(_))) {
                Readiness::NotInstalled
            } else {
                Readiness::Ready
            },
            models: vec![],
            account: None,
        }
    }
}
#[derive(Clone)]
pub enum Spawner {
    Cli(Box<env::ChildEnv>),
    #[cfg(test)]
    Scripted(scripted::Scripted),
}
impl Spawner {
    pub fn spawn(
        &self,
        provider: ProviderKind,
        spec: SessionSpec,
        out: mpsc::Sender<EngineMsg>,
        epoch: u32,
    ) -> Result<SessionHandle, ProviderError> {
        match self {
            Self::Cli(env) => {
                session::spawn_provider(provider, spec, env.as_ref().clone(), out, epoch)
            }
            #[cfg(test)]
            Self::Scripted(script) => Ok(script.spawn(spec, out, epoch)),
        }
    }
    pub fn terminal(&self) -> Result<TerminalHandle, ProviderError> {
        match self {
            Self::Cli(_) => Err(ProviderError::NotBuilt),
            #[cfg(test)]
            Self::Scripted(script) => Ok(script.terminal()),
        }
    }
}

#[derive(Default)]
pub struct Probes {
    tasks: std::collections::BTreeMap<ProviderKind, ProbeTask>,
}
struct ProbeTask {
    cancel: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
impl Probes {
    pub fn check(
        &mut self,
        runtime: &tokio::runtime::Handle,
        provider: ProviderKind,
        env: env::ChildEnv,
        cwd: PathBuf,
    ) -> tokio::sync::oneshot::Receiver<Probe> {
        if env.closing.is_closing() {
            let (_, result) = tokio::sync::oneshot::channel();
            return result;
        }
        let previous = self.tasks.remove(&provider);
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let (reply, result) = tokio::sync::oneshot::channel();
        let task = runtime.spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.cancel.send(());
                let _ = previous.task.await;
            }
            if reply.is_closed() || session::check_cancelled(&mut cancelled).is_err() {
                return;
            }
            let probe = provider.probe_cancellable(&env, &cwd, cancelled).await;
            env.catalog.lock().unwrap().insert(provider, probe.clone());
            let _ = reply.send(probe);
        });
        self.tasks.insert(provider, ProbeTask { cancel, task });
        result
    }
}
impl Drop for Probes {
    fn drop(&mut self) {
        for (_, task) in std::mem::take(&mut self.tasks) {
            let _ = task.cancel.send(());
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ActiveTurn {
    Veyra(TurnId),
    Provider(TurnId),
}
impl ActiveTurn {
    pub fn target(self) -> TurnId {
        match self {
            Self::Veyra(turn) | Self::Provider(turn) => turn,
        }
    }
    pub fn id(self) -> Option<TurnId> {
        match self {
            Self::Veyra(turn) => Some(turn),
            Self::Provider(_) => None,
        }
    }
}
enum StopPhase {
    Protocol(tokio::time::Instant),
    Terminate(tokio::time::Instant),
}
pub(crate) struct StopLadder {
    phase: StopPhase,
    pub reason: String,
}
impl StopLadder {
    pub fn new(reason: StopReason) -> Self {
        Self {
            phase: StopPhase::Protocol(
                tokio::time::Instant::now() + std::time::Duration::from_secs(3),
            ),
            reason: reason.text().into(),
        }
    }
    pub fn deadline(&self) -> tokio::time::Instant {
        match self.phase {
            StopPhase::Protocol(at) | StopPhase::Terminate(at) => at,
        }
    }
    pub fn escalate(&mut self) -> Option<crate::platform::Signal> {
        if tokio::time::Instant::now() < self.deadline() {
            return None;
        }
        Some(match self.phase {
            StopPhase::Protocol(_) => {
                self.phase = StopPhase::Terminate(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(1),
                );
                crate::platform::Signal::Terminate
            }
            StopPhase::Terminate(_) => crate::platform::Signal::Kill,
        })
    }
    pub fn outcome(&self) -> TurnOutcome {
        TurnOutcome::Interrupted {
            reason: self.reason.clone(),
        }
    }
}
pub(crate) enum Outbound {
    Prompt(serde_json::Value),
    Message(serde_json::Value),
    Provider(Box<crate::mcp::ThreadFrame>),
    Interrupt {
        turn: TurnId,
        line: serde_json::Value,
    },
}
impl Outbound {
    #[cfg(any(debug_assertions, test))]
    pub fn value(&self) -> serde_json::Value {
        match self {
            Self::Provider(frame) => frame.value(),
            Self::Prompt(line) | Self::Message(line) | Self::Interrupt { line, .. } => line.clone(),
        }
    }
    pub fn into_bytes(self) -> serde_json::Result<Vec<u8>> {
        match self {
            Self::Provider(frame) => (*frame).into_bytes(),
            Self::Prompt(line) | Self::Message(line) | Self::Interrupt { line, .. } => {
                serde_json::to_vec(&line)
            }
        }
    }
}
#[derive(Default)]
pub(crate) struct Decoded {
    pub events: Vec<ProviderEvent>,
    pub replies: Vec<serde_json::Value>,
    pub controls: Vec<Outbound>,
    pub provider_turn: Option<TurnId>,
    #[cfg(debug_assertions)]
    pub recording: Option<crate::record::Metadata>,
}

impl Decoded {
    pub fn take_outbound(&mut self) -> Vec<Outbound> {
        let mut frames: Vec<_> = std::mem::take(&mut self.replies)
            .into_iter()
            .map(Outbound::Message)
            .collect();
        let mut interrupts = Vec::new();
        for frame in std::mem::take(&mut self.controls) {
            if matches!(frame, Outbound::Interrupt { .. }) {
                interrupts.push(frame);
            } else {
                frames.push(frame);
            }
        }
        frames.extend(interrupts);
        frames
    }
}

#[cfg(all(test, debug_assertions))]
mod shutdown_probe_tests {
    use super::*;
    #[tokio::test]
    async fn cancelling_probe_during_version_cleanup_starts_no_initialize() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/probe-cancel-{}",
            crate::domain::ThreadId::new()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let _scratch = crate::tests::Scratch(root.clone());
        let binary = root.join("owned-probe-stand-in");
        std::fs::write(
            &binary,
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
  trap 'printf stopping > version-stopping' TERM
  printf '1.0.0\n'
  while :; do /bin/sleep 0.05; done
fi
printf launched > initialize-spawned
read initialize
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
        let mut env = env::ChildEnv::load();
        env.groups = Default::default();
        env.claude_bin = Some(binary);
        env.fixtures = None;
        env.record = None;
        let mut probes = Probes::default();
        let reply = probes.check(
            &tokio::runtime::Handle::current(),
            ProviderKind::Claude,
            env.clone(),
            root.clone(),
        );
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !root.join("version-stopping").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(probes);
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), reply).await;
        assert!(result.is_ok(), "probe did not finish cleanup");
        assert!(
            env.groups.lock().unwrap().is_empty(),
            "owned groups survived cleanup"
        );
        assert_eq!(
            env.launches.load(std::sync::atomic::Ordering::Acquire),
            1,
            "cancelled probe spawned initialize"
        );
        assert!(
            !root.join("initialize-spawned").exists(),
            "cancelled probe launched initialize after version cleanup"
        );
    }
}

#[cfg(test)]
#[tokio::test(start_paused = true)]
async fn shared_stop_ladder_enforces_deadlines_and_reason() {
    let mut ladder = StopLadder::new(StopReason::User);
    assert!(ladder.escalate().is_none());
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    assert!(matches!(
        ladder.escalate(),
        Some(crate::platform::Signal::Terminate)
    ));
    assert!(ladder.escalate().is_none());
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    assert!(matches!(
        ladder.escalate(),
        Some(crate::platform::Signal::Kill)
    ));
    assert_eq!(
        ladder.outcome(),
        TurnOutcome::Interrupted {
            reason: "Stopped by you".into()
        }
    );
}
#[cfg(test)]
#[test]
fn decoded_batch_always_places_replies_before_interrupts() {
    let turn = TurnId::new();
    let mut decoded = Decoded {
        controls: vec![Outbound::Interrupt {
            turn,
            line: serde_json::json!({"interrupt":true}),
        }],
        replies: vec![serde_json::json!({"decision":"cancel"})],
        ..Default::default()
    };
    assert_eq!(
        decoded
            .take_outbound()
            .iter()
            .map(Outbound::value)
            .collect::<Vec<_>>(),
        vec![
            serde_json::json!({"decision":"cancel"}),
            serde_json::json!({"interrupt":true})
        ]
    );
}
