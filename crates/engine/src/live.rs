use crate::{
    domain::{Millis, ThreadView, Turn, TurnId, TurnState, wait_reason},
    provider::SessionHandle,
};

#[derive(Clone, Debug, Default)]
pub struct LiveThread {
    pub epoch: u32,
    pub live: Live,
    pub background_work: bool,
    pub capabilities: Vec<String>,
    pub admission: Option<TurnAdmission>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnAdmission {
    Pending(TurnId),
    Refused,
}
impl LiveThread {
    pub fn is_active(&self, turn: &Turn) -> bool {
        matches!(turn.state, TurnState::Running { .. })
            || self.pending_turn() == Some(turn.id) && turn.state == TurnState::Queued
    }
    pub fn pending_turn(&self) -> Option<TurnId> {
        match self.admission {
            Some(TurnAdmission::Pending(turn)) => Some(turn),
            Some(TurnAdmission::Refused) | None => None,
        }
    }
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
    Stopping {},
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
        && !view.turn_busy()
        && !view.pending_request
        && !view.live.background_work
        && now - idle_since >= 10 * 60 * 1000
}

pub fn busy(view: &ThreadView<'_>) -> bool {
    view.turn_busy() || view.live.background_work || matches!(view.live.live, Live::Starting { .. })
}
pub fn needs_awake(
    projection: &crate::domain::Projection,
    live: &std::collections::BTreeMap<crate::domain::ThreadId, LiveThread>,
) -> bool {
    busy_threads(projection, live).next().is_some()
}
pub(crate) fn busy_threads<'a>(
    projection: &'a crate::domain::Projection,
    live: &'a std::collections::BTreeMap<crate::domain::ThreadId, LiveThread>,
) -> impl Iterator<Item = crate::domain::ThreadId> + 'a {
    let changes = crate::events::ChangeSet::default();
    projection.threads.keys().copied().filter(move |&id| {
        changes
            .view(projection, live, id)
            .is_some_and(|view| busy(&view))
    })
}
