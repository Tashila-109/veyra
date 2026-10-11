use crate::{
    domain::{Millis, ThreadView, TurnId, wait_reason},
    provider::{SessionHandle, TerminalHandle},
};

#[derive(Clone, Debug, Default)]
pub struct LiveThread {
    pub epoch: u32,
    pub live: Live,
    pub background_work: bool,
    pub capabilities: Vec<String>,
}
#[derive(Clone, Debug, Default)]
pub enum Live {
    #[default]
    Parked,
    Starting {
        session: Option<SessionHandle>,
    },
    Structured {
        session: SessionHandle,
        pid: u32,
        pgid: u32,
        draining: Option<TurnId>,
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
impl Live {
    pub fn session(&self) -> Option<&SessionHandle> {
        match self {
            Self::Starting { session } => session.as_ref(),
            Self::Structured { session, .. } => Some(session),
            _ => None,
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
