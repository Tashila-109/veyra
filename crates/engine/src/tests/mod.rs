mod perf;
mod recovery;
mod session_events;
mod storage;
mod table;

use crate::{
    actor::{Actor, EngineMsg, now},
    commands::{Ack, Command, CommandError},
    domain::*,
    events::{ChangeSet, UiEvent},
    live::{AfterStop, Live, LiveState, LiveThread},
    provider::{scripted::Scripted, *},
    store,
};
use rusqlite::Connection;
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot},
};

struct Fixture {
    actor: Actor,
    runtime: Runtime,
    thread: ThreadId,
    project: ProjectId,
    controls: mpsc::UnboundedReceiver<SessionControl>,
    events: mpsc::UnboundedReceiver<UiEvent>,
}
fn config() -> ThreadConfig {
    ThreadConfig {
        model: None,
        effort: None,
        mode: PermissionMode::FullAccess,
    }
}
impl Fixture {
    fn new(state: LiveState) -> Self {
        let runtime = Runtime::new().unwrap();
        let (out, _rx) = mpsc::channel(256);
        let mut db = Connection::open_in_memory().unwrap();
        store::initialize(&mut db).unwrap();
        let mut actor = Actor::new(db, out, Spawner::Scripted(Scripted::default())).unwrap();
        let project = Project {
            id: ProjectId::new(),
            path: "/tmp".into(),
            name: "Test project".into(),
        };
        let thread = Thread {
            id: ThreadId::new(),
            project: project.id,
            title: "A test conversation".into(),
            lineage: Lineage::Root,
            provider: ProviderKind::Claude,
            config: config(),
            cwd: "/tmp".into(),
            native: None,
            native_cursor: None,
            cli_version: None,
            hold: None,
            view: if state == LiveState::Terminal {
                View::Terminal
            } else {
                View::Chat
            },
            import_pending: false,
            archived: false,
        };
        let (control, controls) = mpsc::unbounded_channel();
        let session = SessionHandle { control };
        let live = match state {
            LiveState::Parked => Live::Parked,
            LiveState::Starting => Live::Starting {
                session: Some(session),
            },
            LiveState::Idle | LiveState::Running => Live::Structured {
                session,
                pid: 9,
                pgid: 9,
            },
            LiveState::Terminal => {
                let (control, _rx) = mpsc::unbounded_channel();
                Live::Terminal {
                    term: TerminalHandle { control },
                    pid: 9,
                }
            }
            LiveState::Stopping => Live::Stopping {
                then: AfterStop::StartTerminal,
            },
        };
        let mut c = ChangeSet::default();
        c.projects.insert(project.id, project.clone());
        c.threads.insert(thread.id, thread.clone());
        c.live.insert(
            thread.id,
            LiveThread {
                epoch: 1,
                live,
                background_work: false,
                capabilities: vec![],
            },
        );
        let tx = actor.db.transaction().unwrap();
        store::apply(&tx, &c, now()).unwrap();
        tx.commit().unwrap();
        c.install(&mut actor.projection, &mut actor.live);
        let (tx, events) = mpsc::unbounded_channel();
        actor.handle(EngineMsg::Subscribe(tx));
        let mut f = Self {
            actor,
            runtime,
            thread: thread.id,
            project: project.id,
            controls,
            events,
        };
        if state == LiveState::Running {
            f.turn(TurnOrigin::Provider, TurnState::Running { since: now() });
        }
        f
    }
    fn save(&mut self, c: ChangeSet) {
        let tx = self.actor.db.transaction().unwrap();
        store::apply(&tx, &c, now()).unwrap();
        tx.commit().unwrap();
        c.install(&mut self.actor.projection, &mut self.actor.live);
    }
    fn turn(&mut self, origin: TurnOrigin, state: TurnState) -> TurnId {
        let mut c = ChangeSet::default();
        let id = crate::commands::queue(&self.actor.projection, &mut c, self.thread, origin);
        c.turns.get_mut(&id).unwrap().state = state;
        self.save(c);
        id
    }
    fn user(&mut self, text: &str) -> TurnId {
        let mut c = ChangeSet::default();
        let id =
            crate::commands::queue_user(&self.actor.projection, &mut c, self.thread, text.into());
        self.save(c);
        id
    }
    fn hold(&mut self, hold: Option<HoldReason>) {
        let mut c = ChangeSet::default();
        let mut t = self.actor.projection.threads[&self.thread].clone();
        t.hold = hold;
        c.threads.insert(t.id, t);
        self.save(c);
    }
    fn command(&mut self, command: Command) -> Result<Ack, CommandError> {
        let _entered = self.runtime.enter();
        let (reply, rx) = oneshot::channel();
        self.actor.handle(EngineMsg::Command { command, reply });
        rx.blocking_recv().unwrap()
    }
    fn event(&mut self, event: ProviderEvent) {
        self.event_epoch(1, event);
    }
    fn event_epoch(&mut self, epoch: u32, event: ProviderEvent) {
        let _entered = self.runtime.enter();
        self.actor.handle(EngineMsg::Provider {
            thread: self.thread,
            epoch,
            event,
        });
    }
    fn running(&self) -> Option<TurnId> {
        self.actor
            .projection
            .turns
            .values()
            .find(|t| t.thread == self.thread && matches!(t.state, TurnState::Running { .. }))
            .map(|t| t.id)
    }
    fn req(&mut self) -> RequestId {
        let turn = self.running().unwrap_or_else(|| {
            self.turn(
                TurnOrigin::Provider,
                TurnState::Ended {
                    outcome: TurnOutcome::Completed,
                    started: None,
                    at: now(),
                },
            )
        });
        let r = Request {
            id: RequestId::new(),
            thread: self.thread,
            turn,
            native: NativeRequestRef("approval-one".into()),
            kind: RequestKind::Approval {
                action: ProposedAction {
                    tool: "shell".into(),
                    command: "echo own-test".into(),
                },
                grant_key: None,
            },
            state: RequestState::Pending,
        };
        let id = r.id;
        let mut c = ChangeSet::default();
        c.requests.insert(id, r);
        self.save(c);
        id
    }
    fn drain(&mut self) -> Vec<UiEvent> {
        let mut out = vec![];
        while let Ok(e) = self.events.try_recv() {
            out.push(e);
        }
        out
    }
    fn loaded(&self) -> Projection {
        store::load(&self.actor.db).unwrap()
    }
    fn family(&mut self) -> ThreadId {
        let mut child = self.actor.projection.threads[&self.thread].clone();
        child.id = ThreadId::new();
        child.title = "Child conversation".into();
        child.lineage = Lineage::Child {
            parent: self.thread,
            root: self.thread,
            relationship: Relationship::Subagent,
        };
        let id = child.id;
        let mut c = ChangeSet::default();
        c.threads.insert(id, child);
        c.live.insert(id, LiveThread::default());
        self.save(c);
        id
    }
}
fn turn_event(turn: Option<TurnId>, event: TurnEvent) -> ProviderEvent {
    ProviderEvent::Turn { turn, event }
}
