use crate::{
    domain::*,
    live::{LiveState, LiveThread},
    provider::SessionControl,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChangeEvent {
    ProjectAdded(Project),
    ThreadCreated(Thread),
    ThreadArchived {
        thread: ThreadId,
    },
    ThreadConfigured {
        thread: ThreadId,
        config: ThreadConfig,
    },
    ThreadHoldChanged {
        thread: ThreadId,
        hold: Option<HoldReason>,
    },
    ThreadLiveChanged {
        thread: ThreadId,
        state: LiveState,
    },
    NativeSessionBound {
        thread: ThreadId,
        native: NativeSessionId,
        cli_version: String,
    },
    TurnQueued(Turn),
    TurnStarted(Turn),
    TurnEnded(Turn),
    ItemAdded(Item),
    ItemCompleted(Item),
    RequestOpened(Request),
    RequestClosed(Request),
    TaskStarted(Task),
    TaskSettled(Task),
    TaskDelivered(Task),
    SettingsChanged(Settings),
}
impl ChangeEvent {
    pub fn thread(&self) -> Option<ThreadId> {
        match self {
            Self::ProjectAdded(_) | Self::SettingsChanged(_) => None,
            Self::ThreadCreated(t) => Some(t.id),
            Self::ThreadArchived { thread }
            | Self::ThreadConfigured { thread, .. }
            | Self::ThreadHoldChanged { thread, .. }
            | Self::ThreadLiveChanged { thread, .. }
            | Self::NativeSessionBound { thread, .. } => Some(*thread),
            Self::TurnQueued(t) | Self::TurnStarted(t) | Self::TurnEnded(t) => Some(t.thread),
            Self::ItemAdded(i) | Self::ItemCompleted(i) => Some(i.thread),
            Self::RequestOpened(r) | Self::RequestClosed(r) => Some(r.thread),
            Self::TaskStarted(t) | Self::TaskSettled(t) | Self::TaskDelivered(t) => Some(t.parent),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiEvent {
    Change {
        counter: Option<u64>,
        event: ChangeEvent,
    },
    TextAppended {
        thread: ThreadId,
        item: ItemId,
        counter: u64,
        text: String,
    },
    StorageBlocked {
        error: Option<String>,
    },
}
#[derive(Clone, Debug)]
pub struct ThreadSnapshot {
    pub items: Vec<Item>,
    pub buffered_text: BTreeMap<ItemId, String>,
    pub counter: u64,
}
#[derive(Clone, Debug)]
pub enum Effect {
    Spawn {
        thread: ThreadId,
        import: bool,
    },
    OpenTerminal {
        thread: ThreadId,
    },
    Control {
        thread: ThreadId,
        control: SessionControl,
    },
    StopTerminal {
        thread: ThreadId,
    },
}
#[derive(Default)]
pub struct ChangeSet {
    pub projects: BTreeMap<ProjectId, Project>,
    pub threads: BTreeMap<ThreadId, Thread>,
    pub turns: BTreeMap<TurnId, Turn>,
    pub items: BTreeMap<ItemId, Item>,
    pub requests: BTreeMap<RequestId, Request>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub grants: Vec<(ThreadId, GrantKey)>,
    pub settings: Option<Settings>,
    pub live: BTreeMap<ThreadId, LiveThread>,
    pub events: Vec<ChangeEvent>,
    pub effects: Vec<Effect>,
}
impl ChangeSet {
    pub fn install(
        self,
        p: &mut Projection,
        live: &mut BTreeMap<ThreadId, LiveThread>,
    ) -> (Vec<ChangeEvent>, Vec<Effect>) {
        for t in self.turns.values() {
            if let TurnState::Running { since } = t.state {
                p.turn_started.insert(t.id, since);
            }
        }
        p.projects.extend(self.projects);
        p.threads.extend(self.threads);
        p.turns.extend(self.turns);
        p.items.extend(self.items);
        p.requests.extend(self.requests);
        p.tasks.extend(self.tasks);
        if let Some(settings) = self.settings {
            p.settings = settings;
        }
        live.extend(self.live);
        (self.events, self.effects)
    }
    pub fn thread<'a>(&'a self, p: &'a Projection, id: ThreadId) -> Option<&'a Thread> {
        self.threads.get(&id).or_else(|| p.threads.get(&id))
    }
    pub fn turns<'a>(&'a self, p: &'a Projection, id: ThreadId) -> impl Iterator<Item = &'a Turn> {
        p.turns
            .values()
            .filter(move |t| t.thread == id && !self.turns.contains_key(&t.id))
            .chain(self.turns.values().filter(move |t| t.thread == id))
    }
    pub fn live<'a>(
        &'a self,
        l: &'a BTreeMap<ThreadId, LiveThread>,
        id: ThreadId,
    ) -> &'a LiveThread {
        self.live
            .get(&id)
            .or_else(|| l.get(&id))
            .expect("thread runtime")
    }
    pub fn view<'a>(
        &'a self,
        p: &'a Projection,
        l: &'a BTreeMap<ThreadId, LiveThread>,
        id: ThreadId,
    ) -> Option<ThreadView<'a>> {
        let thread = self.thread(p, id)?;
        let running = self
            .turns(p, id)
            .find(|t| matches!(t.state, TurnState::Running { .. }));
        let queued = self.turns(p, id).any(|t| t.state == TurnState::Queued);
        let last = self.turns(p, id).max_by_key(|t| t.ordinal);
        let pending_request = p
            .requests
            .values()
            .filter(|r| !self.requests.contains_key(&r.id))
            .chain(self.requests.values())
            .any(|r| r.thread == id && r.state == RequestState::Pending);
        Some(ThreadView {
            thread,
            live: self.live(l, id),
            running,
            queued,
            pending_request,
            last,
            last_started: last.and_then(|t| p.turn_started.get(&t.id).copied()),
        })
    }
}
