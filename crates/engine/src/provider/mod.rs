pub mod claude;
pub mod codex;
pub mod env;
pub mod jsonrpc;
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
    Parent,
    StorageBlocked,
    AppClosing,
}
impl StopReason {
    pub fn text(self) -> &'static str {
        match self {
            Self::User => "Stopped by you",
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
            | Self::ReasoningDelta { .. }
            | Self::ToolStarted { .. }
            | Self::Plan(_)
            | Self::RequestOpened { .. } => true,
            Self::Accepted
            | Self::ItemEnded { .. }
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
    #[error("Claude protocol: {0}")]
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
    pub fn is_built(self) -> bool {
        matches!(self, Self::Claude | Self::Codex)
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
        if self.is_built() {
            let result = match self {
                Self::Claude => session::probe(env, cwd, cancelled).await,
                Self::Codex => session::probe_codex(env, cwd, cancelled).await,
                Self::Grok => unreachable!(),
            };
            match result {
                Ok(probe) => probe,
                Err(error) => {
                    tracing::warn!(error = %crate::record::redact(&error.to_string(),None), "provider probe failed");
                    Probe {
                        provider: self,
                        path: env.resolve(self).ok(),
                        version: None,
                        readiness: if matches!(error, ProviderError::NotInstalled(_)) {
                            Readiness::NotInstalled
                        } else {
                            Readiness::Ready
                        },
                        models: vec![],
                        account: None,
                    }
                }
            }
        } else {
            Probe {
                provider: self,
                path: env.resolve(self).ok(),
                version: None,
                readiness: Readiness::Ready,
                models: vec![],
                account: None,
            }
        }
    }
}
#[derive(Clone)]
pub enum Spawner {
    Cli(env::ChildEnv),
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
            Self::Cli(env) if provider.is_built() => {
                session::spawn_provider(provider, spec, env.clone(), out, epoch)
            }
            Self::Cli(_) => Err(ProviderError::NotBuilt),
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
pub(crate) enum Outbound {
    Prompt(serde_json::Value),
    Message(serde_json::Value),
    Interrupt(TurnId),
    RpcInterrupt {
        turn: TurnId,
        line: serde_json::Value,
    },
}
impl Outbound {
    pub fn value(&self) -> serde_json::Value {
        match self {
            Self::Prompt(line) | Self::Message(line) | Self::RpcInterrupt { line, .. } => {
                line.clone()
            }
            Self::Interrupt(_) => serde_json::json!({
                "type":"control_request", "request_id":"veyra-interrupt",
                "request":{"subtype":"interrupt"}
            }),
        }
    }
}
#[derive(Default)]
pub(crate) struct Decoded {
    pub events: Vec<ProviderEvent>,
    pub replies: Vec<serde_json::Value>,
    pub controls: Vec<Outbound>,
    pub provider_turn: Option<TurnId>,
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
