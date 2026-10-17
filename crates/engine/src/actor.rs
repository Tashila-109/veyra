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
    collections::{BTreeMap, BTreeSet, VecDeque},
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
        cursor: Option<String>,
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
        session: Option<crate::mcp::Caller>,
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
    pub(crate) accepted_turns: BTreeSet<TurnId>,
    buffers: BTreeMap<ItemId, TextBuffer>,
    #[cfg(debug_assertions)]
    delta_counts: BTreeMap<ItemId, u64>,
    item_keys: BTreeMap<(ThreadId, u32, TurnId, NativeItemKey), ItemId>,
    idle_since: BTreeMap<ThreadId, Millis>,
    last_ui: Instant,
    flush_scheduled: bool,
    write_scheduled: bool,
    blocked: Option<Blocked>,
    storage_receipt: Option<std::path::PathBuf>,
    recovery_notice: Option<String>,
    #[cfg(test)]
    pub(crate) fail_write: bool,
    #[cfg(test)]
    pub(crate) after_admission: Option<Box<dyn FnOnce() + Send>>,
    #[cfg(test)]
    pub(crate) test_commits: usize,
}
pub fn now() -> Millis {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as Millis
}
fn clear_drain(live: &mut LiveThread, turn: TurnId) {
    if let Live::Structured { draining, .. } = &mut live.live
        && *draining == Some(turn)
    {
        *draining = None;
    }
}
fn notice_turn(
    projection: &Projection,
    changes: &ChangeSet,
    thread: ThreadId,
    kind: &'static str,
) -> Option<TurnId> {
    let turn = changes
        .turns(projection, thread)
        .filter(|turn| turn.state.started().is_some())
        .max_by_key(|turn| {
            (
                matches!(turn.state, TurnState::Running { .. }),
                turn.recency(),
            )
        })
        .map(|turn| turn.id);
    if turn.is_none() {
        tracing::warn!(thread=%thread, notice_kind=kind, "notice has no started turn");
    }
    turn
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
        let storage_receipt = db
            .path()
            .filter(|path| !path.is_empty())
            .and_then(|path| std::path::Path::new(path).parent())
            .map(|root| root.join("run/storage-blocked.json"));
        let mut recovery_notice = None;
        let lost=storage_receipt.as_ref().and_then(|path| {
            let result=crate::platform::optional_text(path).and_then(|text|text.map(|text|serde_json::from_str::<BTreeMap<ThreadId,String>>(&text)
                .map_err(std::io::Error::other)).transpose());
            match result {
                Ok(rows)=>rows,
                Err(error)=> {
                    let destination = path.with_file_name(format!("storage-blocked-lost-{}.json", uuid::Uuid::now_v7()));
                    let kept=crate::platform::quarantine_file(path, &destination).map(|_|destination);
                    match kept {
                        Ok(path)=>tracing::warn!(error_kind=?error.kind(),path=%path.display(),"lost storage receipt kept privately"),
                        Err(keep_error)=>tracing::warn!(error_kind=?error.kind(),%keep_error,"could not preserve lost storage receipt"),
                    }
                    recovery_notice=Some("Veyra could not read its storage recovery receipt. A private copy was kept when possible. Check the provider conversations before sending more work.".into());
                    None
                }
            }
        });
        let c = reconcile::decide(&projection, now());
        let live = projection
            .threads
            .keys()
            .map(|id| (*id, LiveThread::default()))
            .collect();
        let idle_since = projection.threads.keys().map(|id| (*id, now())).collect();
        let closing = match &spawner {
            Spawner::Cli(env) => env.closing.clone(),
            #[cfg(test)]
            Spawner::Scripted(_) => Default::default(),
        };
        let mut actor = Self {
            state: Arc::new(RuntimeState {
                closing,
                catalog: match &spawner {
                    Spawner::Cli(env) => env.catalog.clone(),
                    #[cfg(test)]
                    Spawner::Scripted(_) => Arc::new(std::sync::Mutex::new(
                        ProviderKind::ALL
                            .into_iter()
                            .map(|provider| Probe {
                                provider,
                                path: Some("/scripted".into()),
                                version: None,
                                readiness: Readiness::Ready,
                                models: vec![],
                                account: None,
                            })
                            .map(|probe| (probe.provider, probe))
                            .collect(),
                    )),
                },
                ..Default::default()
            }),
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
            accepted_turns: BTreeSet::new(),
            buffers: BTreeMap::new(),
            #[cfg(debug_assertions)]
            delta_counts: BTreeMap::new(),
            item_keys: BTreeMap::new(),
            idle_since,
            last_ui: Instant::now(),
            flush_scheduled: false,
            write_scheduled: false,
            blocked: None,
            storage_receipt,
            recovery_notice,
            #[cfg(test)]
            fail_write: false,
            #[cfg(test)]
            after_admission: None,
            #[cfg(test)]
            test_commits: 0,
        };
        let mut c = c;
        if let Some(rows) = lost {
            for (thread, what) in rows {
                if let Some(turn) = notice_turn(&actor.projection, &c, thread, "storage_loss") {
                    commands::add_item(
                        &actor.projection,
                        &mut c,
                        Item {
                            id: ItemId::new(),
                            thread,
                            turn,
                            seq: 0,
                            body: ItemBody::Notice(Notice {
                                text: format!(
                                    "Veyra closed while storage was blocked. Thread {thread} may have lost {what}. Check the provider conversation before sending more work."
                                ),
                            }),
                        },
                    );
                }
            }
        }
        actor.prepare_commit(&mut c);
        actor.write(&c)?;
        c.install(&mut actor.projection, &mut actor.live);
        actor.clear_storage_receipt();
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
                    || ((self.blocked.is_none() || self.storage_receipt.is_none())
                        && self
                            .live
                            .values()
                            .all(|live| matches!(live.live, Live::Parked))))
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
        let closing = self.state.closing.is_closing();
        #[cfg(test)]
        if let Some(pause) = self.after_admission.take() {
            pause();
        }
        if closing {
            self.shutdown();
        }
        if self.shutting_down {
            match msg {
                EngineMsg::Command { reply, .. } => {
                    let _ = reply.send(Err(CommandError::Closed));
                    return;
                }
                EngineMsg::Terminal {
                    thread,
                    epoch,
                    event: TerminalEvent::Exited { started, crash },
                } if self.current(thread, epoch) => {
                    self.state.terminals.lock().unwrap().remove(&thread);
                    if self.blocked.is_some() {
                        let live = self.live.get_mut(&thread).unwrap();
                        live.live = Live::Parked;
                        live.background_work = false;
                    } else {
                        let c = self.terminal_exited(thread, started, crash);
                        self.commit_event(c);
                    }
                    return;
                }
                EngineMsg::Provider {
                    cursor: None,
                    thread,
                    epoch,
                    event: ProviderEvent::Session(SessionEvent::Exited { .. }),
                } if self.blocked.is_some() => {
                    if let Some(live) = self
                        .live
                        .get_mut(&thread)
                        .filter(|live| live.epoch == epoch)
                    {
                        live.live = Live::Parked;
                        live.background_work = false;
                    }
                    return;
                }
                EngineMsg::Provider {
                    cursor: None,
                    thread,
                    epoch,
                    event: ProviderEvent::Session(SessionEvent::Exited { crash }),
                } if self.current(thread, epoch) => {
                    let c = self.exited(thread, crash);
                    self.commit_event(c);
                    return;
                }
                EngineMsg::Tick => {
                    if self.blocked.is_some() {
                        self.retry();
                        self.schedule_shutdown_retry();
                    }
                    return;
                }
                EngineMsg::ShutdownDeadline => {
                    if let Some(blocked) = &self.blocked {
                        tracing::error!(changes=%Self::lost_changes(&blocked.changes),"quit deadline reached with storage changes unwritten");
                    }
                    return;
                }
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
                EngineMsg::Tool { call, reply, .. } => {
                    let _ = reply.send(ToolResult {
                        ok: false,
                        output_tail: Some(format!(
                            "database storage is blocked: {}. {}",
                            blocked.error,
                            crate::tools::storage_recovery(&call)
                        )),
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
                cursor,
            } => {
                let c = self.provider(thread, epoch, event);
                if let Some(mut c) = c {
                    if let Some(cursor) = cursor
                        && c.thread(&self.projection, thread)
                            .unwrap()
                            .native_cursor
                            .as_ref()
                            != Some(&cursor)
                    {
                        let mut row = c.thread(&self.projection, thread).unwrap().clone();
                        row.native_cursor = Some(cursor);
                        c.threads.insert(thread, row);
                    }
                    self.commit_event(c);
                }
            }
            EngineMsg::Terminal {
                thread,
                epoch,
                event: TerminalEvent::Ready { pid },
            } => {
                if self.current(thread, epoch)
                    && let Live::Terminal { term, .. } = &self.live[&thread].live
                {
                    let mut live = self.live[&thread].clone();
                    live.live = Live::Terminal {
                        term: term.clone(),
                        pid: Some(pid),
                    };
                    let mut c = ChangeSet::default();
                    c.live.insert(thread, live);
                    self.commit_event(c);
                }
            }
            EngineMsg::Terminal {
                thread,
                epoch,
                event: TerminalEvent::Exited { started, crash },
            } => {
                if self.current(thread, epoch) {
                    let c = self.terminal_exited(thread, started, crash);
                    self.commit_event(c);
                }
            }
            EngineMsg::Tool {
                caller,
                session,
                call,
                reply,
            } => {
                let current = match session {
                    Some(session) => self.current(caller, session.epoch)
                        && self.live.get(&caller).is_some_and(
                            |live| matches!(live.live,Live::Structured{pid,..} if pid==session.pid),
                        )
                        && match &self.spawner {
                            Spawner::Cli(env) => env
                                .mcp
                                .as_ref()
                                .is_some_and(|server| server.allows(caller, session)),
                            #[cfg(test)]
                            Spawner::Scripted(_) => true,
                        },
                    None => match &self.spawner {
                        Spawner::Cli(_) => false,
                        #[cfg(test)]
                        Spawner::Scripted(_) => true,
                    },
                };
                if !current {
                    let _=reply.send(ToolResult{ok:false,output_tail:Some("the calling session ended or was replaced; call list_providers from the current root session's running turn".into())});
                    return;
                }
                let providers = self
                    .state
                    .catalog
                    .lock()
                    .unwrap()
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                let recovery = crate::tools::storage_recovery(&call);
                let result = crate::tools::decide(
                    &self.projection,
                    &self.live,
                    caller,
                    call,
                    &providers,
                    now(),
                )
                .and_then(|(mut c, value)| {
                    self.prepare_commit(&mut c);
                    self.write(&c)?;
                    self.install(c);
                    Ok(value)
                });
                let result = match result {
                    Ok(value) => ToolResult {
                        ok: true,
                        output_tail: Some(value.to_string()),
                    },
                    Err(error) => ToolResult {
                        ok: false,
                        output_tail: Some(match error {
                            CommandError::Store(error) => {
                                format!("database storage write failed: {error}. {recovery}")
                            }
                            other => other.to_string(),
                        }),
                    },
                };
                let _ = reply.send(result);
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
                        .map(|row| Ev::ThreadCreated(Box::new(row))),
                );
                initial.extend(self.projection.turns.values().cloned().map(
                    |turn| match turn.state {
                        TurnState::Queued => Ev::TurnQueued(turn),
                        TurnState::Running { .. } => Ev::TurnStarted(turn),
                        TurnState::Ended { .. } => Ev::TurnEnded(turn),
                    },
                ));
                initial.extend(self.projection.tasks.values().cloned().map(
                    |task| match task.state {
                        TaskState::Running => Ev::TaskStarted(task),
                        TaskState::Settled {
                            delivery: Delivery::Pending { .. },
                            ..
                        } => Ev::TaskSettled(task),
                        TaskState::Settled {
                            delivery: Delivery::Delivered { .. },
                            ..
                        } => Ev::TaskDelivered(task),
                    },
                ));
                initial.extend(self.projection.threads.keys().map(|id| {
                    ChangeSet::default().status_event(&self.projection, &self.live, *id)
                }));
                for event in initial {
                    let counter = event.thread().map(|id| self.next_counter(id));
                    let _ = tx.send(UiEvent::Change { counter, event });
                }
                if let Some(notice) = &self.recovery_notice {
                    let _ = tx.send(UiEvent::RecoveryNotice(notice.clone()));
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
        let changes = self.db.total_changes();
        let tx = self.db.transaction()?;
        store::apply(&tx, c, now())?;
        if tx.total_changes() == changes {
            tx.rollback()?;
            return Ok(());
        }
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
                            SessionControl::InterruptActive(StopReason::StorageBlocked)
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
            self.record_storage_receipt(&c);
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
        self.clear_storage_receipt();
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
        let ended: BTreeSet<_> = c
            .turns
            .values()
            .filter(|turn| matches!(turn.state, TurnState::Ended { .. }))
            .map(|turn| turn.id)
            .collect();
        let completed: BTreeSet<_> = c
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
        self.acknowledge_text(&c);
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
        self.accepted_turns.retain(|turn| !ended.contains(turn));
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
            if self.state.closing.is_closing()
                && matches!(
                    effect,
                    Effect::Spawn { .. } | Effect::StartTurn { .. } | Effect::OpenTerminal { .. }
                )
            {
                continue;
            }
            match effect {
                Effect::Spawn { thread, import } => {
                    let row = &self.projection.threads[&thread];
                    let spec = SessionSpec {
                        thread,
                        cwd: row.cwd.clone(),
                        config: row.config.clone(),
                        native: row.native.clone(),
                        import: (import || row.import_pending)
                            .then(|| self.history_request(thread)),
                        root: matches!(row.lineage, Lineage::Root),
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
                            cursor: None,
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
                Effect::OpenTerminal { thread } => {
                    let row = &self.projection.threads[&thread];
                    let spec = SessionSpec {
                        thread,
                        cwd: row.cwd.clone(),
                        config: row.config.clone(),
                        native: row.native.clone(),
                        import: None,
                        root: false,
                    };
                    let epoch = self.live[&thread].epoch;
                    let colors = *self.state.terminal_colors.lock().unwrap();
                    match self
                        .spawner
                        .terminal(colors, row.provider, spec, self.out.clone(), epoch)
                    {
                        Ok(term) => {
                            self.state
                                .terminals
                                .lock()
                                .unwrap()
                                .insert(thread, term.clone());
                            self.live.get_mut(&thread).unwrap().live =
                                Live::Terminal { term, pid: None };
                            let event = ChangeSet::default().status_event(
                                &self.projection,
                                &self.live,
                                thread,
                            );
                            let counter = self.next_counter(thread);
                            self.publish(UiEvent::Change {
                                counter: Some(counter),
                                event,
                            });
                        }
                        Err(error) => self.post(EngineMsg::Terminal {
                            thread,
                            epoch,
                            event: TerminalEvent::Exited {
                                started: false,
                                crash: Some(crate::record::redact(&error.to_string(), None)),
                            },
                        }),
                    }
                }
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
            ProviderEvent::Session(SessionEvent::StartTurnRefused { turn }) => {
                if self.live[&thread].pending_turn() != Some(turn) {
                    return None;
                }
                self.requeue_start(&mut c, turn);
                let mut live = self.live[&thread].clone();
                live.admission = Some(crate::live::TurnAdmission::Refused);
                clear_drain(&mut live, turn);
                c.live.insert(thread, live);
            }
            ProviderEvent::Session(SessionEvent::ProviderTurnStarted { provider_turn }) => {
                if !matches!(self.live[&thread].live, Live::Structured { .. }) {
                    return None;
                }
                if let Some(turn) = self.live[&thread].pending_turn() {
                    self.requeue_start(&mut c, turn);
                    let mut live = self.live[&thread].clone();
                    clear_drain(&mut live, turn);
                    c.live.insert(thread, live);
                }
                let view = c.view(&self.projection, &self.live, thread).unwrap();
                if view.running.is_some()
                    || matches!(
                        view.live.live,
                        Live::Structured {
                            draining: Some(_),
                            ..
                        }
                    )
                {
                    return Some(c);
                }
                let turn = Turn {
                    id: provider_turn,
                    thread,
                    ordinal: c
                        .turns(&self.projection, thread)
                        .map(|t| t.ordinal)
                        .max()
                        .unwrap_or(0)
                        + 1,
                    origin: TurnOrigin::Provider,
                    state: TurnState::Running { since: now() },
                };
                c.events.push(Ev::TurnStarted(turn.clone()));
                c.turns.insert(turn.id, turn);
                return Some(c);
            }
            ProviderEvent::Session(SessionEvent::History { native, history }) => {
                if self.projection.threads[&thread].native.as_ref() != Some(&native)
                    || !self.projection.threads[&thread].import_pending
                {
                    return None;
                }
                self.import_history(&mut c, thread, history);
            }
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
                if self.projection.threads[&thread].import_pending {
                    self.import_history(
                        &mut c,
                        thread,
                        History {
                            failed: true,
                            ..Default::default()
                        },
                    );
                }
                let mut t = c.thread(&self.projection, thread).unwrap().clone();
                t.native = Some(native.clone());
                if let Some(cli_version) = cli_version {
                    t.cli_version = Some(cli_version);
                }
                let cli_version = t.cli_version.clone();
                if t != self.projection.threads[&thread] {
                    c.threads.insert(thread, t);
                }
                c.events.push(Ev::NativeSessionBound {
                    thread,
                    native,
                    cli_version,
                });
                if self.projection.threads[&thread].view == View::Terminal {
                    commands::control(&mut c, thread, SessionControl::Shutdown);
                    c.live.get_mut(&thread).unwrap().live = Live::Stopping {
                        then: AfterStop::StartTerminal,
                    };
                }
            }
            ProviderEvent::Session(SessionEvent::Notice { text }) => {
                if let Some(turn) = notice_turn(&self.projection, &c, thread, "session") {
                    commands::add_item(
                        &self.projection,
                        &mut c,
                        Item {
                            id: ItemId::new(),
                            thread,
                            turn,
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
                if let Some(id) = turn
                    && self.live[&thread].pending_turn() == Some(id)
                {
                    let mut live = self.live[&thread].clone();
                    live.admission = None;
                    c.live.insert(thread, live);
                    if matches!(self.projection.turns[&id].state, TurnState::Ended { .. }) {
                        let mut live = c.live(&self.live, thread).clone();
                        if let Live::Structured { draining, .. } = &mut live.live {
                            *draining = Some(id);
                        }
                        c.live.insert(thread, live);
                    }
                    if self.projection.turns[&id].state == TurnState::Queued {
                        let mut started = self.projection.turns[&id].clone();
                        started.state = TurnState::Running { since: now() };
                        c.events.push(Ev::TurnStarted(started.clone()));
                        c.turns.insert(id, started);
                    }
                }
                if matches!(event, TurnEvent::TurnEnded { .. })
                    && c.live(&self.live, thread).admission
                        == Some(crate::live::TurnAdmission::Refused)
                {
                    let mut live = c.live(&self.live, thread).clone();
                    live.admission = None;
                    c.live.insert(thread, live);
                }
                let drained = match c.live(&self.live, thread).live {
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
                    let mut live = c.live(&self.live, thread).clone();
                    clear_drain(&mut live, drained.unwrap());
                    c.live.insert(thread, live);
                } else if turn.is_none() && provider_drain && event.opens_provider_turn() {
                    tracing::info!(%thread, "dropped stopped Provider turn tail");
                    return None;
                }
                let running = c
                    .view(&self.projection, &self.live, thread)
                    .unwrap()
                    .running
                    .cloned();
                let current = match (turn, running) {
                    (Some(id), Some(t)) if id == t.id => t,
                    (None, Some(t)) if t.origin == TurnOrigin::Provider => t,
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
    fn requeue_start(&self, c: &mut ChangeSet, id: TurnId) {
        if let Some(turn) = self.projection.turns.get(&id)
            && matches!(turn.state, TurnState::Running { .. })
        {
            let mut queued = turn.clone();
            queued.state = TurnState::Queued;
            c.events.push(Ev::TurnQueued(queued.clone()));
            c.turns.insert(id, queued);
        }
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
                if !self.accepted_turns.insert(turn.id) {
                    return;
                }
                tracing::info!(
                    "turn accepted thread={} turn={} at_ms={}",
                    thread,
                    turn.id,
                    now()
                );
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
            TurnEvent::AsyncQuestions {
                key,
                text,
                questions,
            } => {
                let index = (thread, epoch, turn.id, key);
                if let Some(id) = self.item_keys.get(&index) {
                    if let Some(mut item) = self.projection.items.get(id).cloned()
                        && matches!(item.body, ItemBody::AssistantText { .. })
                    {
                        self.buffers.remove(id);
                        item.body = ItemBody::AsyncQuestions { text, questions };
                        c.events.push(Ev::ItemAdded(item.clone()));
                        c.items.insert(item.id, item);
                    }
                } else {
                    let item = Item {
                        id: ItemId::new(),
                        thread,
                        turn: turn.id,
                        seq: 0,
                        body: ItemBody::AsyncQuestions { text, questions },
                    };
                    self.item_keys.insert(index, item.id);
                    commands::add_item(&self.projection, c, item);
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
            TurnEvent::ToolTitleChanged { key, title } => {
                if let Some(id) = self.item_keys.get(&(thread, epoch, turn.id, key))
                    && let Some(mut item) = self.projection.items.get(id).cloned()
                    && let ItemBody::Tool(tool) = &mut item.body
                {
                    tool.call.name = title;
                    c.events.push(Ev::ItemAdded(item.clone()));
                    c.items.insert(item.id, item);
                }
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
                let granted = kind
                    .grant_key()
                    .is_some_and(|key| self.projection.grants.contains(&(thread, key.clone())));
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
                    c.events.push(Ev::RequestClosed(Box::new(r.clone())));
                    c.effects.push(Effect::Control {
                        thread,
                        control: SessionControl::Answer {
                            native: r.native.clone(),
                            reply: Decision::AllowOnce,
                        },
                    });
                } else {
                    c.events.push(Ev::RequestOpened(Box::new(r.clone())));
                }
                c.requests.insert(r.id, r);
            }
            TurnEvent::TurnEnded { outcome, .. } => {
                commands::end_turn(&self.projection, c, turn.clone(), outcome, now());
                commands::settle_idle_child(&self.projection, &self.live, c, thread, false, now());
            }
        }
    }
    fn import_history(&self, c: &mut ChangeSet, thread: ThreadId, history: History) {
        let failed = history.failed
            || c.thread(&self.projection, thread)
                .unwrap()
                .native_cursor
                .is_none();
        if failed || !history.text.is_empty() {
            let turn = commands::queue(&self.projection, c, thread, TurnOrigin::Provider);
            let body = if failed {
                ItemBody::Notice(Notice {
                    text: IMPORT_NOTICE.into(),
                })
            } else {
                ItemBody::TerminalSpan(TerminalSpan { text: history.text })
            };
            commands::add_item(
                &self.projection,
                c,
                Item {
                    id: ItemId::new(),
                    thread,
                    turn,
                    seq: 0,
                    body,
                },
            );
            let turn = c.turns[&turn].clone();
            commands::end_turn(&self.projection, c, turn, TurnOutcome::Completed, now());
        }
        let mut row = c.thread(&self.projection, thread).unwrap().clone();
        row.native_cursor = history.cursor;
        row.import_pending = false;
        c.threads.insert(thread, row);
    }
    fn history_request(&self, thread: ThreadId) -> HistoryRequest {
        HistoryRequest {
            cursor: self.projection.threads[&thread].native_cursor.clone(),
            sent: self
                .projection
                .turns_for(thread)
                .filter(|turn| !matches!(turn.origin, TurnOrigin::Provider))
                .map(|turn| turn.id.to_string())
                .collect(),
        }
    }
    fn terminal_exited(&self, thread: ThreadId, started: bool, crash: Option<String>) -> ChangeSet {
        let mut c = self.exited(thread, crash.clone());
        let mut row = c.thread(&self.projection, thread).unwrap().clone();
        row.view = View::Chat;
        row.import_pending = started;
        c.threads.insert(thread, row);
        if let Some(message) = crash {
            let turn = commands::queue(&self.projection, &mut c, thread, TurnOrigin::Provider);
            commands::add_item(
                &self.projection,
                &mut c,
                Item {
                    id: ItemId::new(),
                    thread,
                    turn,
                    seq: 0,
                    body: ItemBody::Notice(Notice {
                        text: message.clone(),
                    }),
                },
            );
            let turn = c.turns[&turn].clone();
            commands::end_turn(
                &self.projection,
                &mut c,
                turn,
                TurnOutcome::Failed { message },
                now(),
            );
        }
        if !self.state.closing.is_closing() {
            commands::start_changed(&self.projection, &self.live, &mut c, now());
        }
        c
    }
    fn exited(&self, thread: ThreadId, crash: Option<String>) -> ChangeSet {
        if let Spawner::Cli(env) = &self.spawner
            && let Some(server) = &env.mcp
        {
            server.revoke(thread, self.live[&thread].epoch);
        }
        self.state.terminals.lock().unwrap().remove(&thread);
        let mut c = ChangeSet::default();
        let old = &self.live[&thread];
        let active: Vec<_> = self
            .projection
            .turns_for(thread)
            .filter(|t| old.is_active(t))
            .cloned()
            .collect();
        if active.is_empty() && matches!(old.live, Live::Starting { .. }) {
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
                            .clone()
                            .unwrap_or_else(|| "The session exited before it was ready".into()),
                    },
                    now(),
                );
            }
            if matches!(old.live, Live::Starting { session: None })
                || c.turns(&self.projection, thread)
                    .any(|turn| turn.state == TurnState::Queued)
            {
                commands::set_hold(&self.projection, &mut c, thread, Some(HoldReason::Stopped));
            }
        }
        for t in active {
            let outcome = match crash.as_ref() {
                Some(m) => TurnOutcome::Failed { message: m.clone() },
                None => TurnOutcome::Interrupted {
                    reason: "The session exited".into(),
                },
            };
            commands::end_turn(&self.projection, &mut c, t.clone(), outcome, now());
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
        live.admission = None;
        live.background_work = false;
        c.live.insert(thread, live);

        if !self.state.closing.is_closing() {
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
        }
        commands::settle_idle_child(&self.projection, &self.live, &mut c, thread, false, now());
        if !self.state.closing.is_closing() {
            commands::start_changed(&self.projection, &self.live, &mut c, now());
        }
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
        debug_assert!(
            !self.shutting_down
                || c.effects.iter().all(|effect| matches!(
                    effect,
                    Effect::Control { .. } | Effect::StopTerminal { .. }
                ))
        );
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
                    commands::task_report(&self.projection, c, task.child),
                )
            })
            .collect();
        for (id, (text, item)) in summaries {
            if let TaskState::Settled {
                summary, source, ..
            } = &mut c.tasks.get_mut(&id).unwrap().state
            {
                *summary = text;
                *source = item;
            }
        }
    }
    fn acknowledge_text(&mut self, c: &ChangeSet) {
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
                        pending_turn: view.live.pending_turn(),
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
    fn lost_changes(c: &ChangeSet) -> String {
        let mut rows: BTreeMap<ThreadId, Vec<String>> = BTreeMap::new();
        for item in c.items.values() {
            let kind = match &item.body {
                ItemBody::UserText(_) => "user text",
                ItemBody::AssistantText { .. } => "assistant text",
                ItemBody::AsyncQuestions { .. } => "question card",
                ItemBody::Reasoning { .. } => "reasoning",
                ItemBody::Tool(_) => "tool result",
                ItemBody::Notice(_) => "provider notice",
                ItemBody::Plan(_) => "plan",
                ItemBody::TaskCard(_) => "task card",
                ItemBody::TerminalSpan(_) => "terminal span",
            };
            rows.entry(item.thread)
                .or_default()
                .push(format!("{kind} item {}", item.id));
        }
        for turn in c.turns.values() {
            rows.entry(turn.thread)
                .or_default()
                .push(format!("turn {} state", turn.id));
        }
        for thread in c.threads.keys() {
            rows.entry(*thread)
                .or_default()
                .push("thread metadata".into());
        }
        for request in c.requests.values() {
            rows.entry(request.thread)
                .or_default()
                .push(format!("request {} state", request.id));
        }
        for task in c.tasks.values() {
            rows.entry(task.parent)
                .or_default()
                .push(format!("task {} state and delivery", task.id));
        }
        serde_json::to_string(
            &rows
                .into_iter()
                .map(|(thread, parts)| (thread, parts.join(", ")))
                .collect::<BTreeMap<_, _>>(),
        )
        .unwrap()
    }
    fn record_storage_receipt(&self, c: &ChangeSet) {
        let text = Self::lost_changes(c);
        tracing::error!(changes=%text,"storage changes retained but not written");
        if let Some(path) = &self.storage_receipt
            && let Err(error) = crate::platform::private_text(path, &text)
        {
            tracing::error!(%error,"could not write storage recovery notice");
        }
    }
    fn clear_storage_receipt(&self) {
        if let Some(path) = &self.storage_receipt
            && let Err(error) = crate::platform::remove_optional_file(path)
        {
            tracing::error!(%error,"could not clear storage recovery notice");
        }
    }
    fn schedule_shutdown_retry(&self) {
        if self.shutting_down && self.blocked.is_some() {
            let out = self.out.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                let _ = out.send(EngineMsg::Tick).await;
            });
        }
    }
    fn shutdown(&mut self) {
        if self.shutting_down {
            return;
        }
        self.shutting_down = true;
        self.state.closing.close();
        let mut c = if let Some(blocked) = self.blocked.take() {
            let mut changes = blocked.changes;
            changes.effects.clear();
            self.acknowledge_text(&changes);
            changes
                .clone()
                .install(&mut self.projection, &mut self.live);
            changes
        } else {
            ChangeSet::default()
        };
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
            commands::end_active(
                &self.projection,
                &self.live,
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
            if matches!(live.live, Live::Starting { session: None }) {
                let mut live = live.clone();
                live.live = Live::Parked;
                live.background_work = false;
                c.live.insert(thread, live);
            }
            if let Live::Terminal { .. } = &live.live {
                c.effects.push(Effect::StopTerminal { thread });
            }
        }
        self.commit_event(c);
        if self.blocked.is_none() {
            self.clear_storage_receipt();
        }
        self.schedule_shutdown_retry();
        for live in self.live.values_mut() {
            if matches!(live.live, Live::Starting { session: None }) {
                live.live = Live::Parked;
                live.background_work = false;
            }
        }
        self.update_awake();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quit_between_decision_and_dispatch_still_ends_promptly() {
        for (closing_before_dispatch, fail_write) in [(false, false), (true, false), (true, true)] {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            let entered = runtime.enter();
            let mut db = Connection::open_in_memory().unwrap();
            store::initialize(&mut db).unwrap();
            let (out, messages) = mpsc::channel(256);
            let mut actor =
                Actor::new(db, out.clone(), Spawner::Scripted(Default::default())).unwrap();
            let (reply, mut ack) = oneshot::channel();
            actor.handle(EngineMsg::Command {
                command: Command::AddProject {
                    path: "/tmp".into(),
                },
                reply,
            });
            let Ack::Project(project) = ack.try_recv().unwrap().unwrap() else {
                panic!("missing project")
            };
            let (mut c, ack) = commands::decide(
                &actor.projection,
                &actor.live,
                Command::CreateThread {
                    project,
                    provider: ProviderKind::Claude,
                    config: ThreadConfig {
                        model: None,
                        effort: None,
                        mode: PermissionMode::AskFirst,
                    },
                    text: "sent as quit begins".into(),
                },
                now(),
            )
            .unwrap();
            let Ack::Thread(thread) = ack else {
                panic!("missing thread")
            };
            assert!(c.effects.iter().any(|e| matches!(e, Effect::Spawn { .. })));
            actor.prepare_commit(&mut c);
            actor.write(&c).unwrap();
            if closing_before_dispatch {
                actor.state.closing.close();
            }
            actor.install(c);
            let live = format!("{:?}", std::mem::discriminant(&actor.live[&thread].live));
            let sessionless = matches!(actor.live[&thread].live, Live::Starting { session: None });
            actor.fail_write = fail_write;
            out.try_send(EngineMsg::Shutdown).unwrap();
            drop(entered);
            let handle = runtime.handle().clone();
            let start = Instant::now();
            std::thread::spawn(move || actor.run(messages, handle))
                .join()
                .unwrap();
            let elapsed = start.elapsed();
            eprintln!(
                "closing_before_dispatch={closing_before_dispatch} sessionless_start={sessionless} \
                 fail_write={fail_write} live={live} quit_took={elapsed:?}"
            );
            assert!(
                elapsed < Duration::from_secs(2),
                "quit waited {elapsed:?} for the deadline; thread {thread} stayed Starting with no session"
            );
        }
    }

    #[test]
    fn shutdown_exit_decides_no_restart_or_queued_start() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _entered = runtime.enter();
        for then in [
            AfterStop::Park,
            AfterStop::StartStructured { import: false },
            AfterStop::StartTerminal,
        ] {
            for hold in [None, Some(HoldReason::Interrupted)] {
                let mut db = Connection::open_in_memory().unwrap();
                store::initialize(&mut db).unwrap();
                let (out, _messages) = mpsc::channel(256);
                let mut actor = Actor::new(db, out, Spawner::Scripted(Default::default())).unwrap();
                let (reply, mut ack) = oneshot::channel();
                actor.handle(EngineMsg::Command {
                    command: Command::AddProject {
                        path: "/tmp".into(),
                    },
                    reply,
                });
                let Ack::Project(project) = ack.try_recv().unwrap().unwrap() else {
                    panic!("missing project")
                };
                let (reply, mut ack) = oneshot::channel();
                actor.handle(EngineMsg::Command {
                    command: Command::CreateThread {
                        project,
                        provider: ProviderKind::Claude,
                        config: ThreadConfig {
                            model: None,
                            effort: None,
                            mode: PermissionMode::AskFirst,
                        },
                        text: "queued before quit".into(),
                    },
                    reply,
                });
                let Ack::Thread(thread) = ack.try_recv().unwrap().unwrap() else {
                    panic!("missing thread")
                };
                actor.live.get_mut(&thread).unwrap().live = Live::Stopping { then: then.clone() };
                actor.projection.threads.get_mut(&thread).unwrap().hold = hold;
                let epoch = actor.live[&thread].epoch;
                if hold.is_none() {
                    assert!(!actor.exited(thread, None).effects.is_empty());
                }
                actor.handle(EngineMsg::Shutdown);
                let c = actor.exited(thread, None);
                assert!(
                    c.effects.iter().all(|effect| matches!(
                        effect,
                        Effect::Control { .. } | Effect::StopTerminal { .. }
                    )),
                    "shutdown proposed a start effect"
                );
                assert_eq!(c.live[&thread].epoch, epoch);
                assert!(matches!(c.live[&thread].live, Live::Parked));
                assert!(c.turns.values().all(|turn| turn.state == TurnState::Queued));
                assert!(
                    c.events
                        .iter()
                        .all(|event| !matches!(event, Ev::TurnStarted(_)))
                );
                assert_eq!(
                    actor.projection.threads[&thread].hold,
                    Some(hold.unwrap_or(HoldReason::Restarted))
                );
            }
        }
    }
}
