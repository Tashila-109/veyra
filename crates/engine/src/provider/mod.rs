#[cfg(test)]
pub mod scripted;
use crate::{actor::EngineMsg, domain::*};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
        cli_version: String,
        pid: u32,
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
    },
    ReasoningDelta {
        key: NativeItemKey,
        text: String,
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
    Plan(Vec<PlanStep>),
    RequestOpened {
        native: NativeRequestRef,
        kind: RequestKind,
    },
    TurnEnded {
        outcome: TurnOutcome,
    },
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
    #[error("this provider is not built yet")]
    NotBuilt,
}
impl ProviderKind {
    pub fn spawn(
        self,
        _spec: SessionSpec,
        _out: mpsc::Sender<EngineMsg>,
        _epoch: u32,
    ) -> Result<SessionHandle, ProviderError> {
        match self {
            Self::Claude | Self::Codex | Self::Grok => Err(ProviderError::NotBuilt),
        }
    }
}
#[derive(Clone, Default)]
pub enum Spawner {
    #[default]
    Cli,
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
            Self::Cli => provider.spawn(spec, out, epoch),
            #[cfg(test)]
            Self::Scripted(script) => Ok(script.spawn(spec, out, epoch)),
        }
    }
    pub fn terminal(&self) -> Result<TerminalHandle, ProviderError> {
        match self {
            Self::Cli => Err(ProviderError::NotBuilt),
            #[cfg(test)]
            Self::Scripted(script) => Ok(script.terminal()),
        }
    }
}
