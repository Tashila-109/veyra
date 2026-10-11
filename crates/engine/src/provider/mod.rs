pub mod claude;
pub mod env;
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
    Interrupt,
    Answer {
        native: NativeRequestRef,
        reply: Reply,
    },
    Shutdown,
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
    #[error("probe cancelled")]
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
        if self == Self::Claude {
            match session::probe(env, cwd, cancelled).await {
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
            Self::Cli(env) => match provider {
                ProviderKind::Claude => session::spawn(spec, env.clone(), out, epoch),
                ProviderKind::Codex | ProviderKind::Grok => Err(ProviderError::NotBuilt),
            },
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
        let previous = self.tasks.remove(&provider);
        let (cancel, mut cancelled) = tokio::sync::oneshot::channel();
        let (reply, result) = tokio::sync::oneshot::channel();
        let task = runtime.spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.cancel.send(());
                let _ = previous.task.await;
            }
            if reply.is_closed() || cancelled.try_recv().is_ok() {
                return;
            }
            let probe = provider.probe_cancellable(&env, &cwd, cancelled).await;
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
