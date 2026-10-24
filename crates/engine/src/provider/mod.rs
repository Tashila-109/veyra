#![allow(clippy::disallowed_types)] // Provider outbound envelopes and raw probe metadata are wire JSON.
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ApprovalField {
    Shown,
    Metadata,
    Passthrough { affects_execution: bool },
}

struct ApprovalFields {
    fields: std::collections::BTreeMap<String, ApprovalField>,
}

impl ApprovalFields {
    fn new(value: &serde_json::Value, classify: impl Fn(&str) -> ApprovalField) -> Self {
        Self {
            fields: value
                .as_object()
                .into_iter()
                .flat_map(|fields| fields.keys())
                .map(|key| (key.clone(), classify(key)))
                .collect(),
        }
    }
    fn residual(&self, value: &serde_json::Value) -> serde_json::Value {
        let mut residual = value.clone();
        if let Some(fields) = residual.as_object_mut() {
            fields.retain(|key, _| matches!(self.fields[key], ApprovalField::Passthrough { .. }));
        }
        residual
    }
    fn preview(&self, value: &serde_json::Value) -> ApprovalPreview {
        let residual = self.residual(value);
        ApprovalPreview::new(
            if residual.as_object().is_some_and(|fields| fields.is_empty()) {
                String::new()
            } else {
                serde_json::to_string_pretty(&residual).expect("JSON approval arguments")
            },
        )
    }
    fn permits_grant(&self) -> bool {
        !self.fields.values().any(|field| {
            matches!(
                field,
                ApprovalField::Passthrough {
                    affects_execution: true
                }
            )
        })
    }
}

#[cfg(test)]
fn approval_wire_sample(
    codec: &str,
    kind: &str,
    wire: serde_json::Value,
    sample: serde_json::Value,
) -> serde_json::Value {
    fn check(
        wire: &serde_json::Value,
        sample: &serde_json::Value,
        path: &str,
        paths: &mut Vec<String>,
    ) {
        if let Some(fields) = wire.as_object() {
            for (key, value) in fields {
                let path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let actual = sample
                    .get(key)
                    .unwrap_or_else(|| panic!("wire field needs a sample: {path}"));
                assert!(!actual.is_null(), "wire field must be planted: {path}");
                paths.push(path.clone());
                check(value, actual, &path, paths);
            }
            for key in sample
                .as_object()
                .unwrap()
                .keys()
                .filter(|key| !fields.contains_key(*key))
            {
                paths.push(if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                });
            }
        } else if let Some(first) = wire.as_array().and_then(|values| values.first()) {
            check(first, &sample[0], &format!("{path}[]"), paths);
        } else if wire.is_null()
            && let Some(fields) = sample.as_object()
        {
            paths.extend(fields.keys().map(|key| format!("{path}.{key}")));
        }
    }
    let mut paths = Vec::new();
    check(&wire, &sample, "", &mut paths);
    eprintln!(
        "APPROVAL_FIELDS {}",
        serde_json::json!({"codec":codec,"kind":kind,"fields":paths,"wire":wire})
    );
    sample
}

#[cfg(test)]
#[test]
fn grant_documentation_names_the_supported_fields_and_background_boundary() {
    let architecture = include_str!("../../../../ARCHITECTURE.md");
    for (prefix, fields) in [
        (
            "Claude offers an exact-command thread grant",
            &["command", "description", "reason", "timeout"][..],
        ),
        (
            "Codex offers a command grant",
            &[
                "threadId",
                "turnId",
                "itemId",
                "startedAtMs",
                "environmentId",
                "kind",
                "commandActions",
                "availableDecisions",
                "proposedExecpolicyAmendment",
            ][..],
        ),
        (
            "A request with rawInput.variant Bash",
            &[
                "description",
                "reason",
                "is_background:false",
                "true or invalid",
            ][..],
        ),
    ] {
        let rule = architecture
            .lines()
            .find(|line| line.starts_with(prefix))
            .expect("grant rule must remain documented");
        for field in fields {
            assert!(rule.contains(field), "grant rule omits {field}: {rule}");
        }
    }
}

#[cfg(test)]
fn assert_approval_preview(action: &ProposedAction, expected: serde_json::Value, field: &str) {
    let preview = match &action.target {
        ApprovalTarget::Command { preview, .. }
        | ApprovalTarget::File { preview, .. }
        | ApprovalTarget::Tool { preview } => preview,
    };
    let text = if expected.as_object().is_some_and(|fields| fields.is_empty()) {
        String::new()
    } else {
        serde_json::to_string_pretty(&expected).unwrap()
    };
    assert_eq!(preview.text, text, "preview carrying {field}");
    assert!(!preview.truncated);
}

#[cfg(test)]
fn approval_test_action(decoded: Decoded) -> ProposedAction {
    assert!(decoded.replies.is_empty());
    decoded
        .events
        .into_iter()
        .find_map(|event| match event {
            ProviderEvent::Turn {
                event:
                    TurnEvent::RequestOpened {
                        kind: RequestKind::Approval { action },
                        ..
                    },
                ..
            } => Some(action),
            _ => None,
        })
        .expect("approval must open")
}

fn approval_explanation<'a>(
    fields: impl IntoIterator<Item = (&'static str, Option<&'a str>)>,
) -> Option<(&'static str, ApprovalReason)> {
    fields.into_iter().find_map(|(field, text)| {
        let mut preview = ApprovalReason::new(text?.into());
        preview.explanation = Some(if field == "description" {
            ApprovalExplanation::Description
        } else {
            ApprovalExplanation::Reason
        });
        Some((field, preview))
    })
}

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
    pub root: bool,
}
#[derive(Clone, Debug)]
pub struct SessionHandle {
    pub control: mpsc::UnboundedSender<SessionControl>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionControl {
    DispatchCommitted {
        turn: TurnId,
    },
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
    Dispatched {
        turn: TurnId,
    },
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
    Notice(Notice),
    BackgroundWork {
        pending: bool,
    },
    ResolutionFailed {
        error: String,
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
        tool: Option<NativeItemKey>,
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
    pub(crate) control: mpsc::UnboundedSender<TerminalControl>,
    pub(crate) terminal: std::sync::Arc<crate::terminal::Terminal>,
}
impl TerminalHandle {
    pub fn close(&self) {
        self.terminal.close();
        let _ = self.control.send(TerminalControl::Stop);
    }
    pub fn is_closing(&self) -> bool {
        self.terminal.is_closing()
    }
    pub fn pid(&self) -> Option<u32> {
        self.terminal.pid()
    }
    pub fn status(&self) -> crate::terminal::ShellStatus {
        self.terminal.status()
    }
    pub fn running_processes(&self) -> Vec<std::path::PathBuf> {
        self.terminal.running_processes()
    }
    pub fn same(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.terminal, &other.terminal)
    }
    pub fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.terminal.write(bytes)
    }
    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.terminal.subscribe()
    }
    pub fn emulator(&self) -> &alacritty_terminal::sync::FairMutex<crate::terminal::Emulator> {
        &self.terminal.emulator
    }
    pub fn resize(&self, size: portable_pty::PtySize) -> std::io::Result<()> {
        self.terminal.resize(size)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalControl {
    Stop,
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
    pub fn offered_modes(self) -> &'static [PermissionMode] {
        match self {
            Self::Claude | Self::Codex => &[
                PermissionMode::AskFirst,
                PermissionMode::AcceptEdits,
                PermissionMode::FullAccess,
            ],
            Self::Grok => &[PermissionMode::AskFirst, PermissionMode::FullAccess],
        }
    }
    pub fn run_mode(self, stored: PermissionMode) -> PermissionMode {
        if self.offered_modes().contains(&stored) {
            stored
        } else {
            PermissionMode::AskFirst
        }
    }
    pub fn next_mode(self, stored: PermissionMode) -> PermissionMode {
        let modes = self.offered_modes();
        let current = self.run_mode(stored);
        let index = modes.iter().position(|mode| *mode == current).unwrap_or(0);
        modes[(index + 1) % modes.len()]
    }
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
    ) -> SessionHandle {
        match self {
            Self::Cli(env) => {
                session::spawn_provider(provider, spec, env.as_ref().clone(), out, epoch)
            }
            #[cfg(test)]
            Self::Scripted(script) => script.spawn(spec, out, epoch),
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
