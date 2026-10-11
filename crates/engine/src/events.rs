use crate::{domain::*, live::LiveThread, provider::SessionControl};
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
    ThreadStatusChanged {
        thread: ThreadId,
        status: Status,
    },
    NativeSessionBound {
        thread: ThreadId,
        native: NativeSessionId,
        cli_version: Option<String>,
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
            | Self::NativeSessionBound { thread, .. }
            | Self::ThreadStatusChanged { thread, .. } => Some(*thread),
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
        #[cfg(debug_assertions)]
        delta_count: u64,
    },
    StorageBlocked {
        error: Option<String>,
    },
}
#[derive(Clone, Debug)]
pub struct ThreadSnapshot {
    pub items: Vec<Item>,
    pub buffered_text: BTreeMap<ItemId, String>,
    #[cfg(debug_assertions)]
    pub delta_counts: BTreeMap<ItemId, u64>,
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
    StartTurn {
        thread: ThreadId,
        turn: TurnId,
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
fn overlay<'a, K: Ord, V>(
    stored: &'a BTreeMap<K, V>,
    pending: &'a BTreeMap<K, V>,
) -> impl Iterator<Item = &'a V> {
    stored
        .iter()
        .filter(|(id, _)| !pending.contains_key(id))
        .map(|(_, row)| row)
        .chain(pending.values())
}

impl ChangeSet {
    pub fn stores_nothing(&self) -> bool {
        let Self {
            projects,
            threads,
            turns,
            items,
            requests,
            tasks,
            grants,
            settings,
            live,
            events: _,
            effects: _,
        } = self;
        projects.is_empty()
            && threads.is_empty()
            && turns.is_empty()
            && items.is_empty()
            && requests.is_empty()
            && tasks.is_empty()
            && grants.is_empty()
            && settings.is_none()
            && live.is_empty()
    }

    pub fn status_event(
        &self,
        projection: &Projection,
        live: &BTreeMap<ThreadId, LiveThread>,
        thread: ThreadId,
    ) -> ChangeEvent {
        ChangeEvent::ThreadStatusChanged {
            thread,
            status: thread_status(&self.view(projection, live, thread).expect("status thread")),
        }
    }
    pub fn install(
        self,
        p: &mut Projection,
        live: &mut BTreeMap<ThreadId, LiveThread>,
    ) -> (Vec<ChangeEvent>, Vec<Effect>) {
        let mut events = self
            .events
            .iter()
            .cloned()
            .map(|event| match event {
                ChangeEvent::ProjectAdded(row) => {
                    ChangeEvent::ProjectAdded(self.projects[&row.id].clone())
                }
                ChangeEvent::ThreadCreated(row) => {
                    ChangeEvent::ThreadCreated(self.threads[&row.id].clone())
                }
                ChangeEvent::TurnQueued(row) => {
                    ChangeEvent::TurnQueued(self.turns[&row.id].clone())
                }
                ChangeEvent::TurnStarted(row) => {
                    ChangeEvent::TurnStarted(self.turns[&row.id].clone())
                }
                ChangeEvent::TurnEnded(row) => ChangeEvent::TurnEnded(self.turns[&row.id].clone()),
                ChangeEvent::ItemAdded(row) => ChangeEvent::ItemAdded(self.items[&row.id].clone()),
                ChangeEvent::ItemCompleted(row) => {
                    ChangeEvent::ItemCompleted(self.items[&row.id].clone())
                }
                ChangeEvent::RequestOpened(row) => {
                    ChangeEvent::RequestOpened(self.requests[&row.id].clone())
                }
                ChangeEvent::RequestClosed(row) => {
                    ChangeEvent::RequestClosed(self.requests[&row.id].clone())
                }
                ChangeEvent::TaskStarted(row) => {
                    ChangeEvent::TaskStarted(self.tasks[&row.id].clone())
                }
                ChangeEvent::TaskSettled(row) => {
                    ChangeEvent::TaskSettled(self.tasks[&row.id].clone())
                }
                ChangeEvent::TaskDelivered(row) => {
                    ChangeEvent::TaskDelivered(self.tasks[&row.id].clone())
                }
                other => other,
            })
            .collect::<Vec<_>>();
        let changed: std::collections::BTreeSet<_> = self
            .live
            .keys()
            .copied()
            .chain(self.turns.values().map(|turn| turn.thread))
            .chain(self.threads.keys().copied())
            .chain(self.requests.values().map(|request| request.thread))
            .collect();
        for id in changed {
            events.push(self.status_event(p, live, id));
        }
        p.projects.extend(self.projects);
        p.threads.extend(self.threads);
        for turn in self.turns.into_values() {
            p.insert_turn(turn);
        }
        for item in self.items.into_values() {
            p.insert_item(item);
        }
        p.requests.extend(self.requests);
        p.grants.extend(self.grants);
        p.tasks.extend(self.tasks);
        if let Some(settings) = self.settings {
            p.settings = settings;
        }
        live.extend(self.live);
        (events, self.effects)
    }
    pub fn thread<'a>(&'a self, p: &'a Projection, id: ThreadId) -> Option<&'a Thread> {
        self.threads.get(&id).or_else(|| p.threads.get(&id))
    }
    pub fn turns<'a>(&'a self, p: &'a Projection, id: ThreadId) -> impl Iterator<Item = &'a Turn> {
        p.turns_for(id)
            .filter(|t| !self.turns.contains_key(&t.id))
            .chain(self.turns.values().filter(move |t| t.thread == id))
    }
    pub fn items<'a>(&'a self, p: &'a Projection, id: ThreadId) -> impl Iterator<Item = &'a Item> {
        p.items_for(id)
            .filter(|i| !self.items.contains_key(&i.id))
            .chain(self.items.values().filter(move |i| i.thread == id))
    }
    pub fn requests<'a>(
        &'a self,
        p: &'a Projection,
        id: ThreadId,
    ) -> impl Iterator<Item = &'a Request> {
        overlay(&p.requests, &self.requests).filter(move |r| r.thread == id)
    }
    pub fn tasks<'a>(&'a self, p: &'a Projection) -> impl Iterator<Item = &'a Task> {
        overlay(&p.tasks, &self.tasks)
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
        let pending_request = self
            .requests(p, id)
            .any(|r| r.state == RequestState::Pending);
        Some(ThreadView {
            thread,
            live: self.live(l, id),
            running,
            queued,
            pending_request,
            last,
        })
    }
}
