use crate::{
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
    pub(crate) db: Connection,
    pub(crate) projection: Projection,
    pub(crate) live: BTreeMap<ThreadId, LiveThread>,
    pub(crate) spawner: Spawner,
    out: mpsc::Sender<EngineMsg>,
    subscribers: Vec<mpsc::UnboundedSender<UiEvent>>,
    pending_snapshots: Vec<(ThreadId, oneshot::Sender<ThreadSnapshot>)>,
    counters: BTreeMap<ThreadId, u64>,
    buffers: BTreeMap<ItemId, TextBuffer>,
    item_keys: BTreeMap<(ThreadId, u32, TurnId, NativeItemKey), ItemId>,
    idle_since: BTreeMap<ThreadId, Millis>,
    last_ui: Instant,
    flush_scheduled: bool,
    write_scheduled: bool,
    blocked: Option<Blocked>,
    #[cfg(test)]
    pub(crate) fail_write: bool,
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
        mut db: Connection,
        out: mpsc::Sender<EngineMsg>,
        spawner: Spawner,
    ) -> Result<Self, StoreError> {
        let mut projection = store::load(&db)?;
        let c = reconcile::decide(&projection, now());
        let tx = db.transaction()?;
        store::apply(&tx, &c, now())?;
        tx.commit()?;
        let mut live = projection
            .threads
            .keys()
            .map(|id| (*id, LiveThread::default()))
            .collect();
        c.install(&mut projection, &mut live);
        let idle_since = projection.threads.keys().map(|id| (*id, now())).collect();
        Ok(Self {
            db,
            projection,
            live,
            spawner,
            out,
            subscribers: vec![],
            pending_snapshots: vec![],
            counters: BTreeMap::new(),
            buffers: BTreeMap::new(),
            item_keys: BTreeMap::new(),
            idle_since,
            last_ui: Instant::now(),
            flush_scheduled: false,
            write_scheduled: false,
            blocked: None,
            #[cfg(test)]
            fail_write: false,
        })
    }
    pub(crate) fn run(mut self, mut rx: mpsc::Receiver<EngineMsg>, runtime: Handle) {
        let _entered = runtime.enter();
        while let Some(msg) = rx.blocking_recv() {
            if matches!(msg, EngineMsg::Shutdown) {
                self.shutdown();
                break;
            }
            self.handle(msg);
        }
    }
    pub(crate) fn handle(&mut self, msg: EngineMsg) {
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
                for event in initial {
                    let counter = event.thread().map(|id| self.next_counter(id));
                    let _ = tx.send(UiEvent::Change { counter, event });
                }
                self.subscribers.push(tx);
            }
            EngineMsg::Shutdown => self.shutdown(),
        }
    }
    fn write(&mut self, c: &ChangeSet) -> Result<(), StoreError> {
        #[cfg(test)]
        if self.fail_write {
            return Err(StoreError::Invalid("injected storage failure".into()));
        }
        let tx = self.db.transaction()?;
        store::apply(&tx, c, now())?;
        tx.commit()?;
        Ok(())
    }
    fn commit_event(&mut self, mut c: ChangeSet) {
        self.prepare_commit(&mut c);
        if let Err(error) = self.write(&c) {
            for live in self.live.values() {
                if let Live::Starting {
                    session: Some(session),
                }
                | Live::Structured { session, .. } = &live.live
                {
                    let _ = session.control.send(SessionControl::Interrupt);
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
                Effect::Control { thread, control } => match &self.live.get(thread)?.live {
                    Live::Starting {
                        session: Some(session),
                    }
                    | Live::Structured { session, .. } => {
                        Some(Dispatch::Session(session.control.clone(), control.clone()))
                    }
                    _ => None,
                },
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
            .collect();
        let (events, effects) = c.install(&mut self.projection, &mut self.live);
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
                && matches!(&self.spawner, Spawner::Cli)
            {
                crate::provider::env::register_thread(row.id);
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
                    if let Live::Structured { session, .. } = &self.live[&thread].live {
                        let _ = session
                            .control
                            .send(SessionControl::StartTurn { turn, text });
                    }
                }
                Effect::OpenTerminal { thread } => match self.spawner.terminal() {
                    Ok(term) => {
                        self.live.get_mut(&thread).unwrap().live = Live::Terminal { term, pid: 1 };
                        let running = self
                            .projection
                            .turns_for(thread)
                            .any(|turn| matches!(turn.state, TurnState::Running { .. }));
                        let state = self.live[&thread].state(running);
                        let counter = self.next_counter(thread);
                        self.publish(UiEvent::Change {
                            counter: Some(counter),
                            event: Ev::ThreadLiveChanged { thread, state },
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
                let session = match &self.live[&thread].live {
                    Live::Starting {
                        session: Some(session),
                    }
                    | Live::Structured { session, .. } => session.clone(),
                    _ => {
                        tracing::debug!(%thread, "dropped Ready outside active session");
                        return None;
                    }
                };
                let mut live = self.live[&thread].clone();
                live.live = Live::Structured { session, pid, pgid };
                live.capabilities = capabilities;
                c.live.insert(thread, live);
                let mut t = self.projection.threads[&thread].clone();
                t.native = Some(native.clone());
                t.cli_version = Some(cli_version.clone());
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
                    .min_by_key(|turn| turn.ordinal)
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
                let running = self
                    .projection
                    .turns_for(thread)
                    .find(|t| matches!(t.state, TurnState::Running { .. }))
                    .cloned();
                let current = match (turn, running) {
                    (Some(id), Some(t)) if id == t.id => t,
                    (None, Some(t)) if t.origin == TurnOrigin::Provider => t,
                    (None, None)
                        if matches!(
                            event,
                            TurnEvent::Started
                                | TurnEvent::TextDelta { .. }
                                | TurnEvent::ReasoningDelta { .. }
                                | TurnEvent::ToolStarted { .. }
                                | TurnEvent::Plan(_)
                                | TurnEvent::RequestOpened { .. }
                        ) =>
                    {
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
                        return None;
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
            TurnEvent::TextDelta { key, text } => {
                self.text_delta(c, (thread, epoch, turn.id, key), text, false)
            }
            TurnEvent::ReasoningDelta { key, text } => {
                self.text_delta(c, (thread, epoch, turn.id, key), text, true)
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
                    body: ItemBody::Tool(ToolItem { call, result: None }),
                };
                self.item_keys
                    .insert((thread, epoch, turn.id, key), item.id);
                commands::add_item(&self.projection, c, item);
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
                            output_tail: output_tail.map(|s| {
                                s.chars()
                                    .rev()
                                    .take(10000)
                                    .collect::<String>()
                                    .chars()
                                    .rev()
                                    .collect()
                            }),
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
                let r = Request {
                    id: RequestId::new(),
                    thread,
                    turn: turn.id,
                    native,
                    kind,
                    state: RequestState::Pending,
                };
                c.events.push(Ev::RequestOpened(r.clone()));
                c.requests.insert(r.id, r);
            }
            TurnEvent::TurnEnded { outcome } => {
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
        } else if matches!(old.live, Live::Starting { .. }) && crash.is_some() {
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
                        message: crash.clone().unwrap(),
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
    ) {
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
    }
    fn prepare_commit(&self, c: &mut ChangeSet) {
        self.complete_ended_items(c);
    }
    fn complete_ended_items(&self, c: &mut ChangeSet) {
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
            ) {
                continue;
            }
            self.append_buffer(&mut item);
            match &mut item.body {
                ItemBody::AssistantText { done, .. } | ItemBody::Reasoning { done, .. } => {
                    *done = true
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
            counter: *self.counters.get(&thread).unwrap_or(&0),
        }
    }
    fn shutdown(&mut self) {
        for live in self.live.values() {
            match &live.live {
                Live::Starting {
                    session: Some(session),
                }
                | Live::Structured { session, .. } => {
                    let _ = session.control.send(SessionControl::Shutdown);
                }
                Live::Terminal { term, .. } => {
                    let _ = term.control.send(TerminalControl::Stop);
                }
                Live::Parked | Live::Starting { session: None } | Live::Stopping { .. } => {}
            }
        }
    }
}
