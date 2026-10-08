use crate::{
    domain::{Millis, ThreadView, wait_reason},
    provider::{SessionHandle, TerminalHandle},
};

#[derive(Clone, Debug, Default)]
pub struct LiveThread {
    pub epoch: u32,
    pub live: Live,
    pub background_work: bool,
}
#[derive(Clone, Debug, Default)]
pub enum Live {
    #[default]
    Parked,
    Starting {
        session: SessionHandle,
    },
    Structured {
        session: SessionHandle,
        pid: u32,
    },
    Terminal {
        term: TerminalHandle,
        pid: u32,
    },
    Stopping {
        then: AfterStop,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AfterStop {
    Park,
    StartStructured { import: bool },
    StartTerminal,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveState {
    Parked,
    Starting,
    Idle,
    Running,
    Terminal,
    Stopping,
}
impl LiveThread {
    pub fn state(&self, running: bool) -> LiveState {
        match self.live {
            Live::Parked => LiveState::Parked,
            Live::Starting { .. } => LiveState::Starting,
            Live::Structured { .. } if running => LiveState::Running,
            Live::Structured { .. } => LiveState::Idle,
            Live::Terminal { .. } => LiveState::Terminal,
            Live::Stopping { .. } => LiveState::Stopping,
        }
    }
}
pub fn can_start(view: &ThreadView<'_>) -> bool {
    view.queued && wait_reason(view).is_none()
}
pub fn park_due(view: &ThreadView<'_>, idle_since: Millis, now: Millis) -> bool {
    matches!(view.live.live, Live::Structured { .. })
        && view.running.is_none()
        && !view.pending_request
        && !view.live.background_work
        && now - idle_since >= 10 * 60 * 1000
}
