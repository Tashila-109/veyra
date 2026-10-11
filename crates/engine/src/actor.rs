use crate::{
    RuntimeState,
    commands::{self, Ack, Command, CommandError},
    domain::*,
    events::{ChangeEvent as Ev, ChangeSet, Effect, ThreadSnapshot, UiEvent},
    live::{AfterStop, Live, LiveThread},
    provider::*,
    reconcile,
    store::{self, StoreError},
};
use rusqlite::Connection;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
};

pub enum EngineMsg {
    Command {
        command: Command,
        reply: oneshot::Sender<Result<Ack, CommandError>>,
    },
    Snapshot {
        thread: ThreadId,
        reply: oneshot::Sender<ThreadSnapshot>,
    },
    Provider {
        thread: ThreadId,
        epoch: u32,
        event: ProviderEvent,
    },
    Terminal {
        thread: ThreadId,
        epoch: u32,
        event: TerminalEvent,
    },
    Tool {
        caller: ThreadId,
        call: ToolCall,
        reply: oneshot::Sender<ToolResult>,
    },
    Tick,
    TextFlush,
    TextWrite,
    Subscribe(mpsc::UnboundedSender<UiEvent>),
    Shutdown,
    ShutdownDeadline,
}
impl EngineMsg {
    fn kind(&self) -> &'static str {
        match self {
            Self::Command { .. } => "command",
            Self::Snapshot { .. } => "snapshot",
            Self::Provider { .. } => "provider",
            Self::Terminal { .. } => "terminal",
            Self::Tool { .. } => "tool",
            Self::Tick => "tick",
            Self::TextFlush => "text_flush",
            Self::TextWrite => "text_write",
            Self::Subscribe(_) => "subscribe",
            Self::Shutdown => "shutdown",
            Self::ShutdownDeadline => "shutdown_deadline",
        }
    }
}
#[derive(Default)]
struct TextBuffer {
    unpersisted: String,
    pending_ui: String,
}
enum Dispatch {
    Session(mpsc::UnboundedSender<SessionControl>, SessionControl),
    Terminal(mpsc::UnboundedSender<TerminalControl>),
}
struct Blocked {
    changes: ChangeSet,
    error: String,
    deferred: VecDeque<EngineMsg>,
}
pub struct Actor {
    pub(crate) state: Arc<RuntimeState>,
    awake: Option<crate::platform::AwakeGuard>,
    shutting_down: bool,
    pub(crate) db: Connection,
    pub(crate) projection: Projection,
    pub(crate) live: BTreeMap<ThreadId, LiveThread>,
    pub(crate) spawner: Spawner,
    out: mpsc::Sender<EngineMsg>,
    subscribers: Vec<mpsc::UnboundedSender<UiEvent>>,
    pending_snapshots: Vec<(ThreadId, oneshot::Sender<ThreadSnapshot>)>,
    counters: BTreeMap<ThreadId, u64>,
    buffers: BTreeMap<ItemId, TextBuffer>,
    #[cfg(debug_assertions)]
    delta_counts: BTreeMap<ItemId, u64>,
    item_keys: BTreeMap<(ThreadId, u32, TurnId, NativeItemKey), ItemId>,
    idle_since: BTreeMap<ThreadId, Millis>,
    last_ui: Instant,
    flush_scheduled: bool,
    write_scheduled: bool,
    blocked: Option<Blocked>,
    #[cfg(test)]
    pub(crate) fail_write: bool,
    #[cfg(test)]
    pub(crate) test_commits: usize,
}
pub fn now() -> Millis {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as Millis
}
impl Actor {
    #[cfg(test)]
    pub(crate) fn streaming_entries(&self) -> (usize, usize) {
        (self.item_keys.len(), self.buffers.len())
    }
    pub(crate) fn new(
        db: Connection,
        out: mpsc::Sender<EngineMsg>,
        spawner: Spawner,
    ) -> Result<Self, StoreError> {
        let projection = store::load(&db)?;
        let c = reconcile::decide(&projection, now());
        let live = projection
            .threads
            .keys()
            .map(|id| (*id, LiveThread::default()))
            .collect();
        let idle_since = projection.threads.keys().map(|id| (*id, now())).collect();
        let mut actor = Self {
            state: Default::default(),
            awake: None,
            shutting_down: false,
            db,
            projection,
            live,
            spawner,
            out,
            subscribers: vec![],
            pending_snapshots: vec![],
            counters: BTreeMap::new(),
            buffers: BTreeMap::new(),
            #[cfg(debug_assertions)]
            delta_counts: BTreeMap::new(),
            item_keys: BTreeMap::new(),
            idle_since,
            last_ui: Instant::now(),
            flush_scheduled: false,
            write_scheduled: false,
            blocked: None,
            #[cfg(test)]
            fail_write: false,
            #[cfg(test)]
            test_commits: 0,
        };
        let mut c = c;
        actor.prepare_commit(&mut c);
        actor.write(&c)?;
        c.install(&mut actor.projection, &mut actor.live);
        actor.publish_window_threads(actor.projection.threads.keys().copied().collect::<Vec<_>>());
        actor.update_awake();
        Ok(actor)
    }
    pub(crate) fn run(mut self, mut rx: mpsc::Receiver<EngineMsg>, runtime: Handle) {
        let _entered = runtime.enter();
        while let Some(msg) = rx.blocking_recv() {
            let deadline = matches!(msg, EngineMsg::ShutdownDeadline);
            self.handle(msg);
            if self.shutting_down
                && (deadline
                    || self
                        .live
                        .values()
                        .all(|live| matches!(live.live, Live::Parked)))
            {
                break;
            }
        }
    }
    pub(crate) fn handle(&mut self, msg: EngineMsg) {
        let start = Instant::now();
        let kind = msg.kind();
        #[cfg(debug_assertions)]
        {
            self.state.counters.lock().unwrap().started = Some(start);
        }
        self.handle_inner(msg);
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        #[cfg(debug_assertions)]
        {
            let mut counters = self.state.counters.lock().unwrap();
            counters.actor_max_ms = counters.actor_max_ms.max(ms);
            counters.started = None;
        }
        if ms > 50.0 {
            tracing::warn!(kind, elapsed_ms = ms, "engine actor message exceeded 50 ms");
        }
    }
    fn handle_inner(&mut self, msg: EngineMsg) {
        if self.shutting_down {
            match msg {
                EngineMsg::Command { reply, .. } => {
                    let _ = reply.send(Err(CommandError::Closed));
                    return;
                }
                EngineMsg::Provider {
                    thread,
                    epoch,
                    event: ProviderEvent::Session(SessionEvent::Exited { crash }),
                } if self.current(thread, epoch) => {
                    let c = self.exited(thread, crash);
                    self.commit_event(c);
                    return;
                }
                EngineMsg::ShutdownDeadline => return,
                EngineMsg::Snapshot { .. } | EngineMsg::Subscribe(_) => {}
                _ => return,
            }
        }
        if let Some(blocked) = &mut self.blocked {
            match msg {
                EngineMsg::Command { reply, .. } => {
                    let _ = reply.send(Err(CommandError::StorageBlocked(blocked.error.clone())));
                    return;
                }
                EngineMsg::Provider { .. } | EngineMsg::Terminal { .. } => {
                    blocked.deferred.push_back(msg);
                    return;
                }
                EngineMsg::Tool { reply, .. } => {
                    let _ = reply.send(ToolResult {
                        ok: false,
                        output_tail: Some(format!("database is blocked: {}", blocked.error)),
                    });
                    return;
                }
                EngineMsg::Tick => {
                    self.retry();
                    return;
                }
                EngineMsg::TextWrite => {
                    self.write_scheduled = false;
                    return;
                }
                EngineMsg::Snapshot { .. }
                | EngineMsg::Subscribe(_)
                | EngineMsg::Shutdown
                | EngineMsg::ShutdownDeadline
                | EngineMsg::TextFlush => {}
            }
        }
        match msg {
            EngineMsg::Command { command, reply } => {
                #[cfg(test)]
                let measure = matches!(command, Command::Send { .. });
                #[cfg(test)]
                let start = Instant::now();
                let result = commands::decide(&self.projection, &self.live, command, now())
                    .and_then(|(mut c, ack)| {
                        self.prepare_commit(&mut c);
                        self.write(&c)?;
                        #[cfg(test)]
                        if measure && let Spawner::Scripted(script) = &self.spawner {
                            script
                                .timings
                                .lock()
                                .unwrap()
                                .push(start.elapsed().as_nanos());
                        }
                        self.install(c);
                        Ok(ack)
                    });
                let _ = reply.send(result);
            }
            EngineMsg::Snapshot { thread, reply } => {
                let pending = self.buffers.iter().any(|(id, buffer)| {
                    !buffer.pending_ui.is_empty()
                        && self
                            .projection
                            .items
                            .get(id)
                            .is_some_and(|item| item.thread == thread)
                });
                if pending {
                    self.pending_snapshots.push((thread, reply));
                    self.schedule_text();
                } else {
                    let _ = reply.send(self.snapshot(thread));
                }
            }
            EngineMsg::Provider {
                thread,
                epoch,
                event,
            } => {
                let c = self.provider(thread, epoch, event);
                if let Some(c) = c {
                    self.commit_event(c);
                }
            }
            EngineMsg::Terminal {
                thread,
                epoch,
                event: TerminalEvent::Exited,
            } => {
                if self.current(thread, epoch) {
                    let c = self.exited(thread, None);
                    self.commit_event(c);
                }
            }
            EngineMsg::Tool { reply, .. } => {
                let _ = reply.send(ToolResult {
                    ok: false,
                    output_tail: Some("agent tools are not built yet".into()),
                });
            }
            EngineMsg::Tick => {
                let mut c = ChangeSet::default();
                for &id in self.projection.threads.keys() {
                    commands::idle_park_tick(
                        &self.projection,
                        &self.live,
                        &mut c,
                        id,
                        *self.idle_since.get(&id).unwrap_or(&now()),
                        now(),
                    );
                }
                self.commit_event(c);
            }
            EngineMsg::TextFlush => {
                self.flush_scheduled = false;
                self.flush_ui();
                for (thread, reply) in std::mem::take(&mut self.pending_snapshots) {
                    let _ = reply.send(self.snapshot(thread));
                }
            }
            EngineMsg::TextWrite => {
                self.write_scheduled = false;
                self.write_text();
            }
            EngineMsg::Subscribe(tx) => {
                let _ = tx.send(UiEvent::Change {
                    counter: None,
                    event: Ev::SettingsChanged(self.projection.settings),
                });
                if let Some(blocked) = &self.blocked {
                    let _ = tx.send(UiEvent::StorageBlocked {
                        error: Some(blocked.error.clone()),
                    });
                }
                let mut initial = Vec::new();
                initial.extend(
                    self.projection
                        .projects
                        .values()
                        .cloned()
                        .map(Ev::ProjectAdded),
                );
                initial.extend(
                    self.projection
                        .threads
                        .values()
                        .cloned()
                        .map(Ev::ThreadCreated),
                );
                initial.extend(self.projection.turns.values().cloned().map(
                    |turn| match turn.state {
                        TurnState::Queued => Ev::TurnQueued(turn),
                        TurnState::Running { .. } => Ev::TurnStarted(turn),
                        TurnState::Ended { .. } => Ev::TurnEnded(turn),
                    },
                ));
                initial.extend(self.projection.threads.keys().map(|id| {
                    ChangeSet::default().status_event(&self.projection, &self.live, *id)
                }));
                for event in initial {
                    let counter = event.thread().map(|id| self.next_counter(id));
                    let _ = tx.send(UiEvent::Change { counter, event });
                }
                self.subscribers.push(tx);
            }
            EngineMsg::Shutdown => self.shutdown(),
            EngineMsg::ShutdownDeadline => {}
        }
    }
    fn write(&mut self, c: &ChangeSet) -> Result<(), StoreError> {
        #[cfg(test)]
        if self.fail_write {
            return Err(StoreError::Invalid("injected storage failure".into()));
        }
        if c.stores_nothing() {
            return Ok(());
        }
        let tx = self.db.transaction()?;
        store::apply(&tx, c, now())?;
        tx.commit()?;
        #[cfg(debug_assertions)]
        {
            self.state.counters.lock().unwrap().commits += 1;
        }
        #[cfg(test)]
        {
            self.test_commits += 1;
        }
        Ok(())
    }
    fn commit_event(&mut self, mut c: ChangeSet) {
        self.prepare_commit(&mut c);
        if let Err(error) = self.write(&c) {
            for live in self.live.values() {
                if let Some(session) = live.live.session() {
                    let control =
                        if self.shutting_down || matches!(live.live, Live::Starting { .. }) {
                            SessionControl::Shutdown
                        } else {
                            SessionControl::Interrupt(StopReason::StorageBlocked)
                        };
                    let _ = session.control.send(control);
                }
                if let Live::Terminal { term, .. } = &live.live {
                    let _ = term.control.send(TerminalControl::Stop);
                }
            }
            let error = error.to_string();
            self.publish(UiEvent::StorageBlocked {
                error: Some(error.clone()),
            });
            self.blocked = Some(Blocked {
                changes: c,
                error,
                deferred: VecDeque::new(),
            });
        } else {
            self.install(c);
        }
    }
    fn retry(&mut self) {
        let mut blocked = self.blocked.take().unwrap();
        if let Err(e) = self.write(&blocked.changes) {
            blocked.error = e.to_string();
            self.blocked = Some(blocked);
            return;
        }
        self.install(blocked.changes);
        self.publish(UiEvent::StorageBlocked { error: None });
        while let Some(msg) = blocked.deferred.pop_front() {
            self.handle(msg);
            if let Some(next) = &mut self.blocked {
                next.deferred.extend(blocked.deferred);
                break;
            }
        }
    }
    fn install(&mut self, c: ChangeSet) {
        let controls: Vec<_> = c
            .effects
            .iter()
            .filter_map(|effect| match effect {
                Effect::Control { thread, control } => self
                    .live
                    .get(thread)?
                    .live
                    .session()
                    .map(|session| Dispatch::Session(session.control.clone(), control.clone())),
                Effect::StopTerminal { thread } => match &self.live.get(thread)?.live {
                    Live::Terminal { term, .. } => Some(Dispatch::Terminal(term.control.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        let ended: std::collections::BTreeSet<_> = c
            .turns
            .values()
            .filter(|turn| matches!(turn.state, TurnState::Ended { .. }))
            .map(|turn| turn.id)
            .collect();
        let completed: std::collections::BTreeSet<_> = c
            .items
            .values()
            .filter(|item| {
                ended.contains(&item.turn)
                    || matches!(
                        item.body,
                        ItemBody::AssistantText { done: true, .. }
                            | ItemBody::Reasoning { done: true, .. }
                    )
                    || matches!(&item.body, ItemBody::Tool(tool) if tool.result.is_some())
            })
            .map(|item| item.id)
            .collect();
        for item in c.items.values() {
            if let Some(buffer) = self.buffers.get_mut(&item.id) {
                let stored = match self.projection.items.get(&item.id).map(|item| &item.body) {
                    Some(
                        ItemBody::AssistantText { text, .. } | ItemBody::Reasoning { text, .. },
                    ) => text.len(),
                    _ => 0,
                };
                if let ItemBody::AssistantText { text, .. } | ItemBody::Reasoning { text, .. } =
                    &item.body
                    && text.len() == stored + buffer.unpersisted.len()
                    && text.ends_with(&buffer.unpersisted)
                {
                    buffer.unpersisted.clear();
                }
            }
        }
        let changed: Vec<_> = c
            .turns
            .values()
            .map(|turn| turn.thread)
            .chain(c.live.keys().copied())
            .chain(c.threads.keys().copied())
            .chain(c.requests.values().map(|request| request.thread))
            .collect();
        let (events, effects) = c.install(&mut self.projection, &mut self.live);
        self.publish_window_threads(changed.iter().copied());
        self.update_awake();
        #[cfg(debug_assertions)]
        {
            let mut processes = self.state.processes.lock().unwrap();
            processes.retain(|thread, _| !matches!(self.live[thread].live, Live::Parked));
            for (&thread, live) in &self.live {
                if let Live::Structured { pid, .. } = live.live {
                    processes.insert(thread, pid);
                }
            }
        }

        for id in changed {
            if !self
                .projection
                .turns_for(id)
                .any(|turn| matches!(turn.state, TurnState::Running { .. }))
            {
                self.idle_since.insert(id, now());
            }
        }
        for event in events {
            #[cfg(debug_assertions)]
            if let Ev::ThreadCreated(row) = &event
                && let Spawner::Cli(env) = &self.spawner
            {
                env.register_thread(row.id);
            }
            if matches!(
                event,
                Ev::RequestOpened(_) | Ev::ItemCompleted(_) | Ev::TurnEnded(_)
            ) {
                self.flush_ui();
            }
            let counter = event.thread().map(|id| self.next_counter(id));
            self.publish(UiEvent::Change { counter, event });
        }
        self.item_keys
            .retain(|(_, _, turn, _), id| !ended.contains(turn) && !completed.contains(id));
        self.buffers.retain(|id, _| {
            !completed.contains(id)
                && !self
                    .projection
                    .items
                    .get(id)
                    .is_some_and(|item| ended.contains(&item.turn))
        });
        #[cfg(debug_assertions)]
        self.delta_counts.retain(|id, _| {
            self.projection.items.get(id).is_some_and(|item| {
                self.projection
                    .threads
                    .get(&item.thread)
                    .is_some_and(|thread| !thread.archived)
            })
        });
        self.schedule_text();
        for control in controls {
            match control {
                Dispatch::Session(sender, control) => {
                    let _ = sender.send(control);
                }
                Dispatch::Terminal(sender) => {
                    let _ = sender.send(TerminalControl::Stop);
                }
            }
        }
        for effect in effects {
            match effect {
                Effect::Spawn { thread, import } => {
                    let row = &self.projection.threads[&thread];
                    let spec = SessionSpec {
                        thread,
                        cwd: row.cwd.clone(),
                        config: row.config.clone(),
                        native: row.native.clone(),
                        import,
                    };
                    let epoch = self.live[&thread].epoch;
                    match self
                        .spawner
                        .spawn(row.provider, spec, self.out.clone(), epoch)
                    {
                        Ok(session) => {
                            self.live.get_mut(&thread).unwrap().live = Live::Starting {
                                session: Some(session),
                            }
                        }
                        Err(error) => self.post(EngineMsg::Provider {
                            thread,
                            epoch,
                            event: ProviderEvent::Session(SessionEvent::Exited {
                                crash: Some(error.to_string()),
                            }),
                        }),
                    }
                }
                Effect::StartTurn { thread, turn } => {
                    let text = commands::turn_text(
                        &self.projection,
                        &ChangeSet::default(),
                        &self.projection.turns[&turn],
                    );
                    if let Some(session) = self.live[&thread].live.session() {
                        let _ = session
                            .control
                            .send(SessionControl::StartTurn { turn, text });
                    }
                }
                Effect::OpenTerminal { thread } => match self.spawner.terminal() {
                    Ok(term) => {
                        self.live.get_mut(&thread).unwrap().live = Live::Terminal { term, pid: 1 };
                        let event =
                            ChangeSet::default().status_event(&self.projection, &self.live, thread);
                        let counter = self.next_counter(thread);
                        self.publish(UiEvent::Change {
                            counter: Some(counter),
                            event,
                        });
                    }
                    Err(error) => {
                        let epoch = self.live[&thread].epoch;
                        self.post(EngineMsg::Provider {
                            thread,
                            epoch,
                            event: ProviderEvent::Session(SessionEvent::Exited {
                                crash: Some(error.to_string()),
                            }),
                        });
                    }
                },
                Effect::Control { .. } | Effect::StopTerminal { .. } => {}
            }
        }
    }
    fn post(&self, message: EngineMsg) {
        let out = self.out.clone();
        tokio::spawn(async move {
            let _ = out.send(message).await;
        });
    }
    fn current(&self, thread: ThreadId, epoch: u32) -> bool {
        self.live.get(&thread).is_some_and(|l| l.epoch == epoch)
    }
    fn provider(
        &mut self,
        thread: ThreadId,
        epoch: u32,
        event: ProviderEvent,
    ) -> Option<ChangeSet> {
        if !self.current(thread, epoch) {
            tracing::debug!(%thread,epoch,"dropped stale session event");
            return None;
        }
        let mut c = ChangeSet::default();
        match event {
            ProviderEvent::Session(SessionEvent::Ready {
                native,
                cli_version,
                pid,
                pgid,
                capabilities,
            }) => {
                let session = match self.live[&thread].live.session() {
                    Some(session) => session.clone(),
                    None => {
                        tracing::debug!(%thread, "dropped Ready outside active session");
                        return None;
                    }
                };
                let mut live = self.live[&thread].clone();
                let draining = match live.live {
                    Live::Structured { draining, .. } => draining,
                    _ => None,
                };
                live.live = Live::Structured {
                    session,
                    pid,
                    pgid,
                    draining,
                };
                live.capabilities = capabilities;
                c.live.insert(thread, live);
                let mut t = self.projection.threads[&thread].clone();
                t.native = Some(native.clone());
                if let Some(cli_version) = cli_version {
                    t.cli_version = Some(cli_version);
                }
                let cli_version = t.cli_version.clone();
                t.import_pending = false;
                c.threads.insert(thread, t);
                c.events.push(Ev::NativeSessionBound {
                    thread,
                    native,
                    cli_version,
                });
            }
            ProviderEvent::Session(SessionEvent::Notice { text }) => {
                if let Some(turn) = self
                    .projection
                    .turns_for(thread)
                    .max_by_key(|turn| turn.ordinal)
                {
                    commands::add_item(
                        &self.projection,
                        &mut c,
                        Item {
                            id: ItemId::new(),
                            thread,
                            turn: turn.id,
                            seq: 0,
                            body: ItemBody::Notice(Notice { text }),
                        },
                    );
                }
            }
            ProviderEvent::Session(SessionEvent::BackgroundWork { pending }) => {
                let mut live = self.live[&thread].clone();
                live.background_work = pending;
                c.live.insert(thread, live);
                if !pending {
                    commands::settle_idle_child(
                        &self.projection,
                        &self.live,
                        &mut c,
                        thread,
                        false,
                        now(),
                    );
                }
            }
            ProviderEvent::Session(SessionEvent::Exited { crash }) => {
                return Some(self.exited(thread, crash));
            }
            ProviderEvent::Turn { turn, event } => {
                if !matches!(self.live[&thread].live, Live::Structured { .. }) {
                    tracing::debug!(%thread,"dropped turn outside structured session");
                    return None;
                }
                let drained = match self.live[&thread].live {
                    Live::Structured { draining, .. } => draining,
                    _ => None,
                };
                let provider_drain = drained
                    .is_some_and(|id| self.projection.turns[&id].origin == TurnOrigin::Provider);
                if matches!(event, TurnEvent::TurnEnded { park: true, .. }) {
                    commands::control(&mut c, thread, SessionControl::Shutdown);
                    commands::stopping(&self.live, &mut c, thread, AfterStop::Park);
                } else if matches!(event, TurnEvent::TurnEnded { .. })
                    && drained.is_some()
                    && (turn == drained || turn.is_none() && provider_drain)
                {
                    let mut live = self.live[&thread].clone();
                    if let Live::Structured { draining, .. } = &mut live.live {
                        *draining = None;
                    }
                    c.live.insert(thread, live);
                } else if turn.is_none() && provider_drain && event.opens_provider_turn() {
                    tracing::info!(%thread, "dropped stopped Provider turn tail");
                    return None;
                }
                let running = self
                    .projection
                    .turns_for(thread)
                    .find(|t| matches!(t.state, TurnState::Running { .. }))
                    .cloned();
                let current = match (turn, running) {
                    (Some(id), Some(t)) if id == t.id => t,
                    (None, Some(t)) if t.origin == TurnOrigin::Provider => t,
                    (None, None) if event.opens_provider_turn() => {
                        let id =
                            commands::queue(&self.projection, &mut c, thread, TurnOrigin::Provider);
                        let mut t = c.turns[&id].clone();
                        t.state = TurnState::Running { since: now() };
                        c.turns.insert(id, t.clone());
                        c.events.push(Ev::TurnStarted(t.clone()));
                        t
                    }
                    _ => {
                        tracing::debug!(%thread,"dropped event for non-running turn");
                        commands::start_changed(&self.projection, &self.live, &mut c, now());
                        return Some(c);
                    }
                };
                self.turn_event(&mut c, thread, epoch, current, event);
            }
        }
        commands::start_changed(&self.projection, &self.live, &mut c, now());
        Some(c)
    }
    fn turn_event(
        &mut self,
        c: &mut ChangeSet,
        thread: ThreadId,
        epoch: u32,
        turn: Turn,
        event: TurnEvent,
    ) {
        match event {
            TurnEvent::Started => {}
            TurnEvent::Accepted => {
                for task in self.projection.tasks.values() {
                    let mut t = task.clone();
                    if let TaskState::Settled {
                        delivery: Delivery::Pending { wake },
                        ..
                    } = t.state
                        && wake == turn.id
                    {
                        if let TaskState::Settled { delivery, .. } = &mut t.state {
                            *delivery = Delivery::Delivered { wake };
                        }
                        c.events.push(Ev::TaskDelivered(t.clone()));
                        c.tasks.insert(t.id, t);
                    }
                }
            }
            TurnEvent::TextDelta {
                key,
                text,
                streamed,
            } => {
                let item = self.text_delta(c, (thread, epoch, turn.id, key), text, false);
                #[cfg(not(debug_assertions))]
                let _ = (item, streamed);
                #[cfg(debug_assertions)]
                if streamed {
                    *self.delta_counts.entry(item).or_default() += 1;
                }
            }
            TurnEvent::ReasoningDelta { key, text } => {
                self.text_delta(c, (thread, epoch, turn.id, key), text, true);
            }
            TurnEvent::ItemEnded { key } => {
                if let Some(id) = self.item_keys.get(&(thread, epoch, turn.id, key))
                    && let Some(mut item) = self.projection.items.get(id).cloned()
                {
                    self.append_buffer(&mut item);
                    if let ItemBody::AssistantText { done, .. } | ItemBody::Reasoning { done, .. } =
                        &mut item.body
                    {
                        *done = true;
                    }
                    c.events.push(Ev::ItemCompleted(item.clone()));
                    c.items.insert(item.id, item);
                }
            }
            TurnEvent::ToolStarted { key, call } => {
                let item = Item {
                    id: ItemId::new(),
                    thread,
                    turn: turn.id,
                    seq: 0,
                    body: ItemBody::Tool(ToolItem {
                        call,
                        result: None,
                        output_tail: String::new(),
                    }),
                };
                self.item_keys
                    .insert((thread, epoch, turn.id, key), item.id);
                commands::add_item(&self.projection, c, item);
            }
            TurnEvent::ToolOutput { key, text } => {
                if let Some(id) = self.item_keys.get(&(thread, epoch, turn.id, key))
                    && let Some(mut item) = self.projection.items.get(id).cloned()
                    && let ItemBody::Tool(tool) = &mut item.body
                {
                    tool.output_tail.push_str(&text);
                    tool.output_tail = tool_tail(std::mem::take(&mut tool.output_tail));
                    c.events.push(Ev::ItemAdded(item.clone()));
                    c.items.insert(item.id, item);
                }
            }
            TurnEvent::ToolFinished {
                key,
                ok,
                output_tail,
            } => {
                if let Some(id) = self.item_keys.get(&(thread, epoch, turn.id, key))
                    && let Some(mut i) = self.projection.items.get(id).cloned()
                {
                    if let ItemBody::Tool(t) = &mut i.body {
                        t.result = Some(ToolResult {
                            ok,
                            output_tail: if t.output_tail.is_empty() {
                                output_tail.map(tool_tail)
                            } else {
                                let mut text = std::mem::take(&mut t.output_tail);
                                if let Some(output) = output_tail {
                                    text.push_str(&output);
                                }
                                Some(tool_tail(text))
                            },
                        });
                    }
                    c.events.push(Ev::ItemCompleted(i.clone()));
                    c.items.insert(i.id, i);
                }
            }
            TurnEvent::Plan(steps) => commands::add_item(
                &self.projection,
                c,
                Item {
                    id: ItemId::new(),
                    thread,
                    turn: turn.id,
                    seq: 0,
                    body: ItemBody::Plan(steps),
                },
            ),
            TurnEvent::RequestOpened { native, kind } => {
                let granted = matches!(&kind, RequestKind::Approval { grant_key: Some(key), .. }
                    if self.projection.grants.contains(&(thread, key.clone())));
                let r = Request {
                    id: RequestId::new(),
                    thread,
                    turn: turn.id,
                    native,
                    kind,
                    state: if granted {
                        RequestState::Answered(Decision::AllowOnce)
                    } else {
                        RequestState::Pending
                    },
                };
                if granted {
                    c.events.push(Ev::RequestClosed(r.clone()));
                    c.effects.push(Effect::Control {
                        thread,
                        control: SessionControl::Answer {
                            native: r.native.clone(),
                            reply: Decision::AllowOnce,
                        },
                    });
                } else {
                    c.events.push(Ev::RequestOpened(r.clone()));
                }
                c.requests.insert(r.id, r);
            }
            TurnEvent::TurnEnded { outcome, .. } => {
                commands::end_turn(&self.projection, c, turn.clone(), outcome, now());
                commands::settle_idle_child(&self.projection, &self.live, c, thread, false, now());
            }
        }
    }
    fn exited(&self, thread: ThreadId, crash: Option<String>) -> ChangeSet {
        let mut c = ChangeSet::default();
        let old = &self.live[&thread];
        let running = self
            .projection
            .turns_for(thread)
            .find(|t| matches!(t.state, TurnState::Running { .. }))
            .cloned();
        if let Some(t) = running {
            let outcome = match crash.as_ref() {
                Some(m) => TurnOutcome::Failed { message: m.clone() },
                None => TurnOutcome::Interrupted {
                    reason: "The session exited".into(),
                },
            };
            commands::end_turn(&self.projection, &mut c, t.clone(), outcome, now());
        } else if matches!(old.live, Live::Starting { .. }) {
            if let Some(t) = self
                .projection
                .turns_for(thread)
                .filter(|t| t.state == TurnState::Queued)
                .min_by_key(|t| t.ordinal)
            {
                commands::end_turn(
                    &self.projection,
                    &mut c,
                    t.clone(),
                    TurnOutcome::Failed {
                        message: crash
                            .unwrap_or_else(|| "The session exited before it was ready".into()),
                    },
                    now(),
                );
            }
            commands::set_hold(&self.projection, &mut c, thread, Some(HoldReason::Stopped));
        }
        for req in self
            .projection
            .requests
            .values()
            .filter(|r| r.thread == thread && r.state == RequestState::Pending)
        {
            commands::expire_requests(&self.projection, &mut c, req.turn, "The session exited");
        }
        let mut live = old.clone();
        live.live = Live::Parked;
        live.background_work = false;
        c.live.insert(thread, live);

        match &old.live {
            Live::Stopping {
                then: AfterStop::StartStructured { import },
            } => commands::spawn(&self.live, &mut c, thread, *import),
            Live::Stopping {
                then: AfterStop::StartTerminal,
            } => {
                c.live.get_mut(&thread).unwrap().epoch += 1;
                c.effects.push(Effect::OpenTerminal { thread });
            }
            Live::Parked
            | Live::Starting { .. }
            | Live::Structured { .. }
            | Live::Terminal { .. }
            | Live::Stopping {
                then: AfterStop::Park,
            } => {}
        }
        commands::settle_idle_child(&self.projection, &self.live, &mut c, thread, false, now());
        commands::start_changed(&self.projection, &self.live, &mut c, now());
        c
    }
    fn text_delta(
        &mut self,
        c: &mut ChangeSet,
        index: (ThreadId, u32, TurnId, NativeItemKey),
        text: String,
        reasoning: bool,
    ) -> ItemId {
        let (thread, _, turn, _) = index;
        let id = match self.item_keys.get(&index) {
            Some(id) => *id,
            None => {
                let id = ItemId::new();
                self.item_keys.insert(index, id);
                commands::add_item(
                    &self.projection,
                    c,
                    Item {
                        id,
                        thread,
                        turn,
                        seq: 0,
                        body: if reasoning {
                            ItemBody::Reasoning {
                                text: String::new(),
                                done: false,
                            }
                        } else {
                            ItemBody::AssistantText {
                                text: String::new(),
                                done: false,
                            }
                        },
                    },
                );
                id
            }
        };
        let b = self.buffers.entry(id).or_default();
        b.unpersisted.push_str(&text);
        b.pending_ui.push_str(&text);
        id
    }
    fn prepare_commit(&self, c: &mut ChangeSet) {
        if self.shutting_down {
            // Exit decisions can still propose starts while sessions retire during Quit.
            c.effects.retain(|effect| {
                match effect {
                    Effect::Spawn { thread, .. } | Effect::OpenTerminal { thread } => {
                        let live = c.live.get_mut(thread).unwrap();
                        live.epoch = self.live[thread].epoch;
                        live.live = Live::Parked;
                        live.background_work = false;
                    }
                    Effect::StartTurn { turn, .. } => {
                        c.turns.get_mut(turn).unwrap().state = TurnState::Queued;
                    }
                    Effect::Control { .. } | Effect::StopTerminal { .. } => return true,
                }
                false
            });
            c.events
                .retain(|event| !matches!(event, Ev::TurnStarted(_)));
        }
        let ended: std::collections::BTreeSet<_> = c
            .turns
            .values()
            .filter(|t| matches!(t.state, TurnState::Ended { .. }))
            .map(|t| t.id)
            .collect();
        let threads: std::collections::BTreeSet<_> = c
            .turns
            .values()
            .filter(|t| ended.contains(&t.id))
            .map(|t| t.thread)
            .collect();
        let items: Vec<_> = threads
            .iter()
            .flat_map(|id| c.items(&self.projection, *id))
            .filter(|i| ended.contains(&i.turn))
            .cloned()
            .collect();
        for mut item in items {
            if !matches!(
                item.body,
                ItemBody::AssistantText { done: false, .. }
                    | ItemBody::Reasoning { done: false, .. }
            ) && !matches!(&item.body, ItemBody::Tool(tool) if tool.result.is_none())
            {
                continue;
            }
            self.append_buffer(&mut item);
            match &mut item.body {
                ItemBody::AssistantText { done, .. } | ItemBody::Reasoning { done, .. } => {
                    *done = true
                }
                ItemBody::Tool(tool) => {
                    tool.result = Some(ToolResult {
                        ok: false,
                        output_tail: Some("The turn ended before this tool returned".into()),
                    });
                }
                _ => {}
            }
            let index = c
                .events
                .iter()
                .position(|e| matches!(e,Ev::TurnEnded(t) if t.id==item.turn))
                .unwrap_or(c.events.len());
            c.events.insert(index, Ev::ItemCompleted(item.clone()));
            c.items.insert(item.id, item);
        }
        let summaries: Vec<_> = c
            .tasks
            .values()
            .filter(|task| {
                matches!(task.state, TaskState::Settled { .. })
                    && self
                        .projection
                        .tasks
                        .get(&task.id)
                        .is_some_and(|old| old.state == TaskState::Running)
            })
            .map(|task| {
                (
                    task.id,
                    commands::task_summary(&self.projection, c, task.child),
                )
            })
            .collect();
        for (id, text) in summaries {
            if let TaskState::Settled { summary, .. } = &mut c.tasks.get_mut(&id).unwrap().state {
                *summary = text;
            }
        }
    }
    fn append_buffer(&self, item: &mut Item) {
        if let Some(b) = self.buffers.get(&item.id) {
            match &mut item.body {
                ItemBody::AssistantText { text, .. } | ItemBody::Reasoning { text, .. } => {
                    text.push_str(&b.unpersisted)
                }
                _ => {}
            }
        }
    }
    fn write_text(&mut self) {
        let mut c = ChangeSet::default();
        for (&id, b) in &self.buffers {
            if b.unpersisted.is_empty() {
                continue;
            }
            if let Some(mut item) = self.projection.items.get(&id).cloned() {
                self.append_buffer(&mut item);
                c.items.insert(id, item);
            }
        }
        if !c.items.is_empty() {
            self.commit_event(c);
        }
    }
    fn schedule_text(&mut self) {
        if !self.flush_scheduled && self.buffers.values().any(|b| !b.pending_ui.is_empty()) {
            self.flush_scheduled = true;
            let out = self.out.clone();
            let wait = Duration::from_millis(16).saturating_sub(self.last_ui.elapsed());
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                let _ = out.send(EngineMsg::TextFlush).await;
            });
        }
        if !self.write_scheduled && self.buffers.values().any(|b| !b.unpersisted.is_empty()) {
            self.write_scheduled = true;
            let out = self.out.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let _ = out.send(EngineMsg::TextWrite).await;
            });
        }
    }
    fn flush_ui(&mut self) {
        let deltas: Vec<_> = self
            .buffers
            .iter_mut()
            .filter(|(_, b)| !b.pending_ui.is_empty())
            .filter_map(|(id, b)| {
                self.projection
                    .items
                    .get(id)
                    .map(|i| (i.thread, *id, std::mem::take(&mut b.pending_ui)))
            })
            .collect();
        for (thread, item, text) in deltas {
            let counter = self.next_counter(thread);
            self.publish(UiEvent::TextAppended {
                thread,
                item,
                counter,
                text,
                #[cfg(debug_assertions)]
                delta_count: self.delta_counts.get(&item).copied().unwrap_or_default(),
            });
        }
        self.last_ui = Instant::now();
    }
    fn next_counter(&mut self, thread: ThreadId) -> u64 {
        let count = self.counters.entry(thread).or_default();
        *count += 1;
        *count
    }
    fn publish(&mut self, event: UiEvent) {
        self.subscribers.retain(|tx| tx.send(event.clone()).is_ok());
    }
    fn snapshot(&self, thread: ThreadId) -> ThreadSnapshot {
        let mut items: Vec<_> = self.projection.items_for(thread).cloned().collect();
        items.sort_by_key(|i| (self.projection.turns[&i.turn].ordinal, i.seq));
        let buffered_text = items
            .iter()
            .filter_map(|i| {
                self.buffers
                    .get(&i.id)
                    .filter(|b| !b.unpersisted.is_empty())
                    .map(|b| (i.id, b.unpersisted.clone()))
            })
            .collect();
        ThreadSnapshot {
            items,
            buffered_text,
            #[cfg(debug_assertions)]
            delta_counts: self
                .projection
                .items_for(thread)
                .filter_map(|item| {
                    self.delta_counts
                        .get(&item.id)
                        .map(|count| (item.id, *count))
                })
                .collect(),
            counter: *self.counters.get(&thread).unwrap_or(&0),
        }
    }
    fn publish_window_threads(&self, changed: impl IntoIterator<Item = ThreadId>) {
        let changes = ChangeSet::default();
        let mut threads = self.state.window_threads.lock().unwrap();
        for id in changed {
            if let Some(view) = changes.view(&self.projection, &self.live, id) {
                threads.insert(
                    id,
                    crate::WindowThread {
                        wait_reason: wait_reason(&view),
                        busy: crate::live::busy(&view),
                    },
                );
            }
        }
    }
    fn update_awake(&mut self) {
        let needed = crate::live::needs_awake(&self.projection, &self.live);
        if needed && self.awake.is_none() {
            match crate::platform::hold_awake(std::process::id()) {
                Ok(guard) => self.awake = Some(guard),
                Err(error) => tracing::warn!(%error, "could not hold idle sleep"),
            }
        } else if !needed {
            self.awake = None;
        }
        self.state
            .awake
            .store(self.awake.is_some(), Ordering::Release);
    }
    fn shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.state.closing.store(true, Ordering::Release);
        let mut c = ChangeSet::default();
        for request in self
            .projection
            .requests
            .values()
            .filter(|r| r.state == RequestState::Pending)
        {
            commands::expire_requests(&self.projection, &mut c, request.turn, "Veyra is closing");
        }
        let active: std::collections::BTreeSet<_> =
            crate::live::busy_threads(&self.projection, &self.live)
                .chain(
                    self.projection
                        .turns
                        .values()
                        .filter(|turn| turn.state == TurnState::Queued)
                        .map(|turn| turn.thread),
                )
                .collect();
        let now = now();
        for thread in active {
            commands::set_hold(
                &self.projection,
                &mut c,
                thread,
                Some(HoldReason::Restarted),
            );
            commands::end_running(
                &self.projection,
                &mut c,
                thread,
                StopReason::AppClosing,
                now,
            );
        }
        for (&thread, live) in &self.live {
            if live.live.session().is_some() {
                c.effects.push(Effect::Control {
                    thread,
                    control: SessionControl::Shutdown,
                });
                let mut live = live.clone();
                live.live = Live::Stopping {
                    then: AfterStop::Park,
                };
                c.live.insert(thread, live);
            }
            if let Live::Terminal { .. } = &live.live {
                c.effects.push(Effect::StopTerminal { thread });
            }
        }
        self.commit_event(c);
        let out = self.out.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let _ = out.send(EngineMsg::ShutdownDeadline).await;
        });
    }
}

fn tool_tail(text: String) -> String {
    text.chars()
        .rev()
        .take(TOOL_OUTPUT_CHARS)
        .collect::<String>()
        .chars()
        .rev()
        .collect()
}
